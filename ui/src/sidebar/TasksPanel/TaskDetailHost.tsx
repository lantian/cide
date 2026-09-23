/**
 * The open task's card as the app mounts it: `TaskDetailModal`, plus the stores and the clock.
 *
 * ```tsx
 * {taskSelected !== null && (
 *   <PanelBoundary name="Task" …><TaskDetailHost /></PanelBoundary>
 * )}
 * ```
 *
 * # Why the card's mount left `TasksPanelHost`
 *
 * It used to live there, beside the list, which meant the card could only exist while the Tasks
 * panel was the sidebar view — and the Agents panel's task links (a run row's second line, and
 * the same chip in Recent) therefore had to *switch the sidebar to Tasks* before anything could
 * appear. The user asked for the smaller gesture: clicking a task under an agent opens the card
 * and moves nothing else. A modal already portals to `document.body` and dims the whole window;
 * yanking the panel out from under it was ceremony, not information.
 *
 * So the mount is `App.tsx`'s now, outside every `sidebar.view` branch, gated only on
 * `tasksStore.selected` — which has always been store state precisely so it survives the panel
 * unmounting. The card opens over whichever panel is showing, from whichever panel selected it,
 * and the Tasks panel keeps exactly the half that is about the *list*: the row of the open task
 * is still marked, because `selected` still reaches it.
 *
 * App gates the mount on `selected !== null` rather than this host returning `null` from inside
 * an always-mounted boundary, and the difference is the boundary: `PanelBoundary` never resets
 * itself, so its close gesture must *unmount* it — clearing the selection collapses the branch,
 * which is what arms the boundary for the next open. The same shape as every sidebar panel.
 *
 * # Why this file exists at all
 *
 * `TaskDetail.tsx` is rendered under node by `ui/scripts/check-agents-render.mjs`, through
 * `react-dom/server`, with nothing stubbed but `window`. That works because it reads **no
 * store, calls no IPC and never reads the clock** — every fact and every gesture arrives as a
 * prop. This host holds the half a server render cannot follow, exactly as `TasksPanelHost`
 * does for the list; its header states the rule at more length.
 *
 * # Two pieces of transient state moved here with the card
 *
 * The field the card has in edit, and the card's armed delete. Both are **transient gesture
 * state** — what is half-typed in this window's box, which button this window is looking at —
 * and both belong to whatever mounts the card, because they are claims about *this card's*
 * controls. `TasksPanelHost` keeps its own `deleteArmed` for the list's row-level delete; the
 * two armings no longer share a value, and that is fine rather than a loss: an arming is a
 * claim about the specific control the user pressed, and carrying it from a row into the card
 * was a coincidence of where the state happened to live, not a behaviour anything relied on.
 *
 * # `nowMs` is a prop, and the clock lives here
 *
 * The comment log prints ages ("4m ago"), so something has to tick — 30 s, `TasksPanelHost`'s
 * cadence and its reasoning: the only thing that moves faster is the seconds figure on a
 * comment under a minute old. Re-armed when the board changes, so a comment the user just
 * wrote reads `0s ago` immediately rather than inheriting the previous tick.
 *
 * # It reads both stores, like the two panel hosts it sits between
 *
 * `runs` and `roles` come from the agents roster — the assignee dropdown and the live-run
 * strip are the card's, so the join moved here with it. The same derivation `TasksPanelHost`
 * still does for the list, and `check-agents-render.mjs` greps both files for it: the seam is
 * the one neither render check mounts, and it shipped an empty-forever dropdown once already.
 */
import { memo, useCallback, useEffect, useMemo, useState, useSyncExternalStore } from 'react'
import { notify, notifyFailure } from '@/chrome/notices'
import { useTasks } from '@/sidebar/tasksStore'
import { useWorkspace } from '@/store/workspace'
import { useAgents } from '@/sidebar/agentsStore'
import { rosterColors, rosterRoles } from '@/sidebar/AgentsPanel/model'
import { TaskDetailModal, TaskDetailPendingModal } from './TaskDetail'
import { useSpec } from '../specStore'
import { followMilestones, milestoneOfTask, useMilestones } from '../milestonesStore'
import { CheckLogModal } from './CheckLogModal'
import {
  attachments as attachmentsApi,
  spec as specApi,
  tasks as tasksApi,
  claudeSend,
  file,
  type ChangeName,
} from '@/ipc/client'
import { AttachmentLightbox } from './AttachmentLightbox'
import { forgetPreview, previewsSnapshot, requestPreview, subscribePreviews } from './attachmentPreviews'
import { onFileDrop, useDropHot } from './fileDrop'
import {
  basename,
  imageAttachmentsOf,
  type StagedAttachment,
  type TaskDetailView,
} from './model'
import { useAwaiting } from '@/panes/awaiting'
import { openTaskSession } from './openSession'
import { errorText } from '@/ipc/errorText'
import { adaptDetail } from './adapt'
import { adaptChange } from '../OpenSpecPanel/adapt'
import { issuesFor, primaryAction } from '../OpenSpecPanel/model'
import {
  draftOf,
  parseTarget,
  compose,
  targetId,
  type RequirementDraft,
} from '../OpenSpecPanel/editModel'
import { acceptNotice, dispatchTargets, type SpecCardView } from './specCard'
import {
  activeEdit,
  armedDelete,
  assigneeHint,
  isLinkKind,
  linkableTargets,
  openTask,
  taskLinks,
} from './model'
import type { ArmedDelete, FieldEdit, LinkChip, LinkTargetOption, RunRef } from './model'
import type { LinkAdd } from './TaskDetail'

/** How often the comment log's ages are recomputed. See the header for why it is not 1 s. */
const TICK_MS = 30_000

/**
 * Memoised like the panel hosts, and for their reason: it is a direct child of `App`, which
 * re-renders on every store notification it subscribes to, and everything the card draws
 * arrives through this host's own subscriptions.
 */
export const TaskDetailHost = memo(TaskDetailHostImpl)

function TaskDetailHostImpl() {
  const board = useTasks((s) => s.board)
  const project = useTasks((s) => s.project)
  const selected = useTasks((s) => s.selected)
  const select = useTasks((s) => s.select)
  const editTask = useTasks((s) => s.edit)
  const removeTask = useTasks((s) => s.remove)

  /*
   * The join with the agents store — see the header. All four selectors return stored values
   * (`check:selectors`' rule); `roles` is derived, so it is memoised on the roster's identity,
   * which `adopt` replaces wholesale on every broadcast.
   */
  const roster = useAgents((s) => s.roster)
  const openRun = useAgents((s) => s.openPane)
  const pauseRun = useAgents((s) => s.pause)
  const resumeRun = useAgents((s) => s.resume)
  const roles = useMemo(() => rosterRoles(roster), [roster])
  /*
   * Every role's colour, by id. (M75) `rosterRoles`' sibling rather than a widening of it — that
   * map is pinned by `check:agents` as id→label and read from three other surfaces — and memoised
   * on the same identity for the same reason: a fresh object out of a selector re-renders for
   * ever and unmounts the root.
   */
  const roleColors = useMemo(() => rosterColors(roster), [roster])
  /*
   * A role's own sentence, for the picker's second line. Derived here rather than added to
   * `rosterRoles`, which is pinned by `check:agents` as an id→label map and read by three other
   * surfaces that want exactly that.
   */
  const roleDescriptions = useMemo(
    () =>
      roster.kind === 'ready'
        ? Object.fromEntries(roster.agents.map((def) => [def.id, def.description]))
        : {},
    [roster],
  )
  /* The console tab is where this project's Claude panes live — `tabs[0]`, the pinned one. */
  const workspaceProject = useWorkspace((state) =>
    project === null ? undefined : state.boot?.workspace.projects[String(project)],
  )
  const addRow = useWorkspace((state) => state.addRow)
  const [sessionNames, setSessionNames] = useState<Readonly<Record<string, string>>>({})
  useEffect(() => {
    // Best effort: a machine where `claude` has never run answers `{}`, which costs each row its
    // name and nothing else — so this never rejects and never blocks the picker.
    void claudeSend.names().then(setSessionNames).catch(() => {})
  }, [])
  /* `RunView` is structurally a `RunRef` — the five fields the chip reads, `phase` opaque. */
  const runs: readonly RunRef[] = roster.kind === 'ready' ? roster.runs : NO_RUNS
  const hint = assigneeHint(roster.kind)

  /**
   * The one field the card has in edit, and what has been typed into it.
   *
   * # Why the draft is here and not in the DOM
   *
   * The card's fields used to be uncontrolled — `defaultValue`, committing on blur — which was
   * right for a form and is wrong for a read-first card: *is this field dirty* is consulted by
   * three separate gestures (opening another field, the scrim, Escape), and a decision that
   * reads the DOM cannot be driven by `check-agents.mjs`. `model.ts` decides all three from this
   * value, and the component executes what it is handed.
   *
   * The objection the old comment raised against controlled fields does not apply. It was about
   * a field controlled *by the board* — `value={task.title}` — which a `tasks-changed` landing
   * mid-sentence would overwrite. This is the user's own text, derived from nothing, so a
   * snapshot cannot touch it; `activeEdit` is what decides when it stops applying, and a new
   * `rev` deliberately is not one of those times.
   *
   * # Cleared when the open task changes, and not when the board does
   *
   * `deleteArmed` below is cleared on any board change because an arming is a claim about a
   * screen that has been replaced. A draft is not: an agent appending a comment to the task
   * somebody is retitling must not take the title away from them. What *does* clear it is the
   * card closing or opening on a different task — including the store clearing `selected` when
   * the open task is deleted by another writer, which is the case this effect exists for.
   * `activeEdit` covers the frame before the effect runs, for the reason `armedDelete`'s pure
   * gate does.
   */
  const [editing, setEditing] = useState<FieldEdit | null>(null)
  useEffect(() => setEditing(null), [selected])

  /**
   * The link picker's draft — which kind, aimed at which task — or `null` at rest. (M30)
   *
   * The `editing` arrangement, for its reason: the card is SSR'd with fixed props, so an open
   * picker must arrive as one. Cleared with the selection — including when a link chip's click
   * *moves* it: a half-picked link aimed at `t-14` must not still be open over `t-15`'s card.
   * Deliberately not cleared on a board change, `editing`'s rule — a picker is the user's own
   * gesture, and an agent commenting elsewhere must not close it.
   */
  const [linkAdd, setLinkAdd] = useState<LinkAdd | null>(null)
  useEffect(() => setLinkAdd(null), [selected])

  /**
   * The delete the user has armed **in the card**, and the board they armed it against. The
   * list's row-level arming is `TasksPanelHost`'s own — see the header for why the split is
   * deliberate. Disarmed whenever the board moves, because an arming is a claim about a screen
   * that has been replaced; `model.ts::armedDelete` refuses the stale pair on the frame before
   * this effect runs.
   */
  const [deleteArmed, setDeleteArmed] = useState<ArmedDelete | null>(null)
  useEffect(() => setDeleteArmed(null), [board])

  /*
   * Attachments. (M39) Three more transients, kept here for the header's reason: the open
   * lightbox, the composer's staged files (controlled from here so a desktop drop can add to
   * them), and the thumbnail cache's snapshot — an object, but the *same* object until an answer
   * lands, which is what `useSyncExternalStore` needs and what `attachmentPreviews.ts` promises.
   */
  const [lightbox, setLightbox] = useState<string | null>(null)
  useEffect(() => setLightbox(null), [selected])
  const [composerStaged, setComposerStaged] = useState<readonly StagedAttachment[]>([])
  useEffect(() => setComposerStaged([]), [selected])
  const previews = useSyncExternalStore(subscribePreviews, previewsSnapshot, previewsSnapshot)
  const dropHot = useDropHot()

  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), TICK_MS)
    return () => clearInterval(timer)
    // `board` on purpose: a new comment should be `0s ago` on the frame it appears, not on the
    // next tick. `newerBoard` keeps the identity stable across dropped snapshots, so a
    // broadcast that changes nothing re-arms nothing.
  }, [board])

  /**
   * Every mutation goes out the same way: fire it, and show the reason if it fails. A gesture
   * that silently does nothing is the failure this project has paid for most often;
   * `TasksPanelHost`'s copy says why the surfacing lives at the gesture and not in the store.
   */
  const guarded = useCallback((done: Promise<void>) => {
    void done.catch(notifyFailure)
  }, [])

  /*
   * The open task, and the edit that still applies to it. `openTask` returns `null` on a board
   * that is not `ready`, which is what keeps the card off screen over an unparseable tracker —
   * and off screen in a window whose board was never attached at all.
   */
  const open = openTask(board, selected)
  const edit = activeEdit(board, open, editing)

  /*
   * The open task's **content**, fetched by id. (M68)
   *
   * The board carries rows now, so a body, a log, a history and a file list are no longer sitting
   * in `open` — `task_get` answers them and this holds the answer. Three things about it are
   * load-bearing.
   *
   * **It is cleared on `selected`, synchronously with the id changing.** A `detail` left standing
   * from the previous task would be drawn under the new task's heading for a frame: `t-15`'s title
   * over `t-14`'s conversation, which is a *wrong* answer that reads as real. The pending card is
   * a true one that lasts one round trip.
   *
   * **It refetches when `rev` moves**, which is how the card sees its own comment appear: every
   * mutation answers with a board whose `rev` is higher, the effect re-runs, and the log comes back
   * with the new line in it. Watching `board` itself would do the same, but `rev` is a number and
   * `newerBoard` already guarantees a dropped snapshot keeps the identity — so this re-runs exactly
   * when the tracker actually moved.
   *
   * **`live` guards the write, not the request.** Two selections in flight can land out of order,
   * and the loser must not overwrite the winner; the flag is cheaper and more certain than
   * comparing ids on the way back, because the id is not the only thing that can have moved.
   */
  const rev = board.kind === 'ready' ? board.rev : null
  const [detail, setDetail] = useState<TaskDetailView | null>(null)
  useEffect(() => setDetail(null), [selected])
  useEffect(() => {
    if (project === null || selected === null) return
    let live = true
    /*
     * `.catch(notifyFailure)` rather than a bare `void`: this runs in an effect, and an unhandled
     * rejection out of one unmounts the tree under React 19 — `TasksPanelHost`'s `guarded` carries
     * the same reason. A refusal leaves `detail` null, so the card stays pending rather than
     * claiming the task is empty.
     */
    void tasksApi
      .get(project, selected)
      .then((wire) => {
        if (live) setDetail(adaptDetail(wire))
      })
      .catch(notifyFailure)
    return () => {
      live = false
    }
  }, [project, selected, rev])

  // Every image on the open card is vouched for once. The cache dedupes, so re-running on each
  // board snapshot costs a loop over the ids and nothing over the IPC.
  useEffect(() => {
    if (detail === null || project === null) return
    for (const image of imageAttachmentsOf(detail))
      requestPreview(String(project), detail.id, image.id)
  }, [detail, project])

  const attachTask = useTasks((s) => s.attachFiles)
  const attachClipboard = useTasks((s) => s.attachClipboard)
  const pickAttachments = useCallback(async (): Promise<readonly StagedAttachment[]> => {
    const paths = await attachmentsApi.pick()
    return paths.map((path) => ({ path, name: basename(path), bytes: null }))
  }, [])
  const stageClipboard = useCallback(async (): Promise<StagedAttachment | null> => {
    const staged = await attachmentsApi.stageClipboard()
    if (staged === null) {
      notify('The clipboard holds no image.', { kind: 'warn' })
      return null
    }
    return { path: staged.path, name: staged.name, bytes: Number(staged.bytes) }
  }, [])

  // A desktop drop on the card, a comment or the composer. The compose dialog's zone is the
  // panel host's; every handler hears every drop and acts on its own kinds.
  useEffect(() => {
    if (open === null) return
    const task = open.id
    return onFileDrop((target, paths) => {
      const files = paths.map((path) => ({ path, name: basename(path), bytes: null }))
      if (target.kind === 'task' && target.task === task) {
        guarded(attachTask(task, { kind: 'task' }, files))
      } else if (target.kind === 'comment' && target.task === task) {
        guarded(attachTask(task, { kind: 'comment', id: target.comment }, files))
      } else if (target.kind === 'composer' && target.task === task) {
        setComposerStaged((staged) => [...staged, ...files])
      }
    })
  }, [open, attachTask, guarded])

  /*
   * The card's link chips and the picker's options, derived per render pass. (M30) Memoised on
   * the board's identity, which `newerBoard` keeps stable across dropped snapshots — and both
   * derivations need the *whole* board, because an incoming edge lives on the other task.
   */
  const links: readonly LinkChip[] = useMemo(
    () => (open === null || board.kind !== 'ready' ? NO_LINKS : taskLinks(open, board.tasks)),
    [open, board],
  )
  const linkTargets = useMemo(
    () =>
      open === null || board.kind !== 'ready'
        ? NO_TARGETS
        : linkableTargets(open.id, board.tasks).map((task) => ({
            id: task.id,
            title: task.title,
            // The status rides along for the search popup's rows: which tasks are already
            // done is half of choosing a blocker.
            status: task.status,
          })),
    [open, board],
  )

  /*
   * The change this task implements, read on demand. (M28)
   *
   * Read here rather than folded into the board, because a change costs three subprocesses:
   * `show`, `status` and `validate`. Fetching one per card that happens to be linked, when it is
   * opened, is a cost the user asked for by opening it; fetching every change on the board would
   * be that cost on every refresh for cards nobody has looked at.
   *
   * It re-reads when the *board* changes, which is what makes the progress bar move while an
   * agent works: `cide://spec-changed` fires on the watcher's route for the run's own worktree,
   * `specStore` adopts it, and this effect asks again.
   */
  const specBoard = useSpec((state) => state.board)
  const [specCard, setSpecCard] = useState<SpecCardView | null>(null)
  /*
   * Why there is no card, once the read has finished. (M28)
   *
   * `null` while the read is in flight, a sentence when it came back with nothing — and the card
   * draws a spinner for the first and the sentence for the second. Before this the read had **no
   * `.catch` at all**: a rejection went nowhere, `specCard` stayed `null`, and there was no way
   * for the card to tell that apart from *still reading*.
   */
  const [specProblem, setSpecProblem] = useState<string | null>(null)
  /**
   * `Integrate & Archive` is in flight. See `TaskDetailProps::specBusy`.
   *
   * Local to the host and not in a store: it is transient gesture state about *this card*, which
   * is the webview's half of the state loop — `ui/src/store/workspace.ts`'s header draws the
   * line, and a durable flag would survive a reload that the merge it describes did not.
   */
  const [specBusy, setSpecBusy] = useState(false)
  /**
   * May this project be asked to write a proposal? (M28)
   *
   * Two facts, and both are needed: the board has to be `ready` (a project with no `openspec/`
   * has nothing to propose *into*), and `.claude/` has to carry a propose command — cide types
   * that command into a conversation and cannot invent it. `SpecBoard::Ready` carries the
   * installed list precisely so a surface can ask this without a subprocess.
   */
  const canPropose =
    specBoard.kind === 'ready' &&
    specBoard.commands.some((command) => command.name === 'propose')
  /*
   * The requirement editor's state. (M28)
   *
   * Held here and not in the card, for the reason every other edit on this card is: the card is
   * SSR'd by the render check with fixed props, so anything stateful in it would be invisible to
   * that gate. `target` is which requirement is open, `draft` is what has been typed, and
   * `problem` is what the last save answered — a refusal is a *state this form draws*, never an
   * error thrown past it, because a failed save has to leave the typing on screen to fix.
   */
  /*
   * The dispatch picker. (M28) Transient gesture state — ADR 0002's one category the webview
   * owns — and here rather than in the card so the card stays a function of its props.
   */
  const [dispatchOpen, setDispatchOpen] = useState(false)
  const [editTarget, setEditTarget] = useState<string | null>(null)
  const [editDraft, setEditDraft] = useState<RequirementDraft | null>(null)
  const [editBusy, setEditBusy] = useState(false)
  const [editProblem, setEditProblem] = useState<
    { kind: 'regressed' | 'conflicted'; messages: readonly string[] } | null
  >(null)
  const change = open?.change ?? null
  /*
   * Does this task's role run in its own checkout? `null` for a task with no role, or a roster
   * nobody has read yet.
   *
   * The fact `primaryAction`'s accept arm names the gesture from — see its fifth parameter. It
   * is a **lookup, not a git call**: `AgentDef::worktree` is already on the wire and already
   * here, so the card can tell an integrate-and-archive from a plain archive without spawning a
   * second `openspec` beside the one the change read costs.
   *
   * `null` for a task handed to a conversation is the whole point: `TaskEdit::SetSession` clears
   * `Task::agent`, so there is no `cide/<role>-<task>` and the press only archives.
   */
  const roleWorktree = useMemo(() => {
    const agent = open?.agent ?? null
    if (agent === null || roster.kind !== 'ready') return null
    return roster.agents.find((def) => def.id === agent)?.worktree ?? null
  }, [roster, open?.agent])

  useEffect(() => {
    if (project === null || change === null) {
      setSpecCard(null)
      setSpecProblem(null)
      return
    }
    let live = true
    /*
     * `null` first, and both of them.
     *
     * A card opened onto a different change must never show the previous one's numbers while the
     * read is in flight — and must not show the previous one's *refusal* either, which is what
     * leaving `specProblem` standing would do: the new change would open reading "the old change
     * could not be read". `spec === null` with no problem is what the card draws as "reading".
     */
    setSpecCard(null)
    setSpecProblem(null)
    // And the in-flight flag, for the same reason: a spinner belongs to the accept that started
    // it, and carrying it onto the next card would claim that card's button was working.
    setSpecBusy(false)
    void specApi
      .change(project, change as ChangeName)
      .then((wire) => {
        if (!live) return
        const view = adaptChange(wire)
        if (view === null) {
          setSpecCard(null)
          setSpecProblem(`${change} could not be read. It may have been archived.`)
          return
        }
        const action = primaryAction(
          view,
          open?.agent ?? null,
          open?.status ?? null,
          open?.session ?? null,
          roleWorktree,
        )
        setSpecCard({
          // `null` here, and filled in below. Whether the pane still exists and whether it is
          // waiting are facts about the *workspace tree* and the *live session*, and both move
          // without the change being re-read — baking them into this snapshot would freeze them
          // at whatever they were when four subprocesses last answered.
          session: null,
          change: view.name,
          // What every other number here has to be read against: an archived change carries
          // `0/0` and a vacuously clean verdict, because a directory listing cannot answer
          // either. See `SpecCardView.archived`.
          archived: view.archivedAs ?? null,
          done: view.completed,
          total: view.total,
          valid: view.validation.valid,
          issues: view.validation.issues.filter((issue) => issue.level.toLowerCase() === 'error')
            .length,
          tasks: view.tasks,
          deltas: view.deltas.map((delta, deltaIndex) => ({
            spec: delta.spec,
            op: delta.op,
            requirements: delta.requirements.map((requirement, index) => ({
              // The address a pencil sends back. Built here, from the same indices the draft is
              // read by, so the requirement a click opens is by construction the one it edits.
              target: targetId({ delta: deltaIndex, requirement: index }),
              // Every validator complaint whose path names this requirement, beside it — and
              // `unattributedIssues` puts the rest in the block's own banner, so each one is drawn
              // exactly once. An issue that fell out of both would be reported by the validator,
              // carried across the wire, and rendered on no screen at all.
              issues: issuesFor(view.validation, requirement.name).map((issue) => issue.message),
              name: requirement.name,
              text: requirement.text,
              scenarios: requirement.scenarios,
              block: requirement.block,
            })),
          })),
          artifacts: view.artifacts.flatMap((artifact) =>
            artifact.existing.map((path) => ({ id: artifact.id, path })),
          ),
          action: {
            id: action.id,
            label: action.label,
            hint: action.hint,
            enabled: action.gate.ok,
            reason: action.gate.ok ? '' : action.gate.reason,
          },
        })
      })
      .catch((error: unknown) => {
        // Not `notifyFailure`: a toast about a panel the user is looking at is worse than the
        // sentence in the panel, and it would fire again on every board move for as long as the
        // card stayed open.
        if (live) setSpecProblem(errorText(error))
      })
    return () => {
      live = false
    }
  }, [project, change, specBoard, open?.agent, open?.status, open?.session, roleWorktree])

  /*
   * Where a run could happen: every role, every Claude conversation this project has open, and
   * one more that does not exist yet.
   *
   * The conversations come from the console tab's own pane tree — `tabs[0]` is the pinned Claude
   * tab — because a session id is a fact about a *pane*, and that is where panes live. Sessions
   * with no `/rename` of their own get a positional label; see `dispatchTargets`.
   */
  const targets = useMemo(() => {
    const consoleTab = workspaceProject?.tabs[0]
    const panes = consoleTab === undefined ? [] : Object.values(consoleTab.tree.panes)
    const sessions = panes
      .filter((pane) => pane.kind === 'claude' && pane.session !== null)
      .map((pane) => ({
        id: String(pane.session),
        // Keyed the way a pane is looked up: `/rename`'s name is stored against the id cide
        // addresses the conversation under, which is the conversation id when there is one.
        name: sessionNames[String(pane.conversation ?? pane.session)] ?? null,
      }))
    return dispatchTargets(roles, roleDescriptions, sessions)
  }, [workspaceProject, roles, roleDescriptions, sessionNames])

  /*
   * Where the conversation this task went to actually is, right now. (M28)
   *
   * `Task::session` records where the work went and **survives the pane closing** — that is
   * deliberate and documented on the Rust field — so "is it still open" cannot be read off the
   * task. It is a walk of the workspace tree, which is where panes live, and it is done here
   * every render rather than cached in `specCard`: the tree moves when somebody closes a pane,
   * and the change is only re-read when four subprocesses answer.
   */
  const sessionPane = useMemo(() => {
    const wanted = open?.session ?? null
    if (wanted === null) return null
    for (const openTab of workspaceProject?.tabs ?? []) {
      for (const [id, node] of Object.entries(openTab.tree.panes)) {
        if (node.kind === 'claude' && String(node.session) === wanted) {
          return { tab: openTab.id, pane: id, conversation: node.conversation ?? node.session }
        }
      }
    }
    return null
  }, [workspaceProject, open?.session])

  /*
   * Hooks are unconditional, so this is called with `null` on every card that has no session and
   * simply answers `false` — `useAwaiting` takes `null | undefined` for exactly this.
   */
  const sessionAwaiting = useAwaiting(open?.session ?? null)

  const sessionRef = useMemo(() => {
    const id = open?.session ?? null
    if (id === null) return null
    return {
      id,
      // The conversation's own `/rename`, or a short form of the id. Never the bare uuid: a row
      // reading `11111111-2222-…` names nothing a person can recognise on screen.
      label: sessionNames[String(sessionPane?.conversation ?? id)] ?? `Conversation ${id.slice(0, 8)}`,
      open: sessionPane !== null,
      awaiting: sessionAwaiting,
    }
  }, [open?.session, sessionPane, sessionNames, sessionAwaiting])

  /*
   * Get back to the conversation a task's work went to, and close the card doing it.
   *
   * The reveal and the close are one gesture, not two calls: choosing a conversation left the
   * modal standing over the pane it had just typed into, so the thing the user asked to see was
   * behind a scrim.
   *
   * `openTaskSession` is where the rest lives — see its header for why the pane is built here and
   * not by a command, and for the ownership flag that makes a resumed pane different from a
   * mirrored one. `mode` decides whether the task is handed back to the conversation or it is
   * merely put on screen.
   *
   * `select(null)` **first**, and only for a gesture that is going to succeed: the reveal raises
   * a window and moves focus, and doing that under a modal scrim is what the close is for. A
   * refusal still reaches `guarded`, so a card that closed and then failed would report into
   * nothing — which is why the close is inside the promise rather than beside it.
   */
  const openSession = useCallback(
    async (id: string, mode: 'open' | 'resume') => {
      if (project === null) return
      select(null)
      await openTaskSession(id, {
        project,
        ...(mode === 'resume' && open !== null ? { task: open.id } : {}),
      })
    },
    [project, select, open],
  )

  /*
   * Which milestone the open task serves (M83), from the store the Milestones tab reads too. The
   * selector returns the store's own `view` object and the chip is derived in a memo — a selector
   * that built the chip would hand back a fresh object every render (`check:selectors`).
   */
  useEffect(() => followMilestones(), [])
  const milestonesView = useMilestones((state) => state.view)
  const [verifyLog, setVerifyLog] = useState(false)
  // A log opened for one task must not greet the next card that opens.
  const openId = open === null ? null : String(open.id)
  useEffect(() => setVerifyLog(false), [openId])
  const verify = useMemo(
    () =>
      open === null
        ? null
        : (milestonesView?.verifies.find((v) => String(v.task) === String(open.id)) ?? null),
    [milestonesView, open],
  )
  const milestone = useMemo(
    () => (open === null ? null : milestoneOfTask(milestonesView, String(open.id))),
    [milestonesView, open],
  )

  if (open === null) return null
  /*
   * The row is in hand and the content is not. Draw the row's own facts and say so — see
   * `TaskDetailPendingModal`, and `TaskDetailView`'s doc for why the alternative (build the card
   * from a row and let the empty values speak) is four separate false claims about the task.
   */
  if (detail === null) {
    return <TaskDetailPendingModal task={open} onClose={() => select(null)} />
  }
  return (
    <>
    <TaskDetailModal
      /*
       * Keyed on the task id. Nothing in the card is uncontrolled any more except the comment
       * composer — and that is precisely what the key is for: half a comment aimed at `t-14`
       * must not still be in the box when `t-15` opens.
       */
      key={open.id}
      task={detail}
      milestone={milestone}
      verify={verify}
      onOpenVerifyLog={() => setVerifyLog(true)}
      onOpenMilestone={(id) => {
        // The card closes first: it is a modal over everything, and the milestone's own modal
        // opens in the Tasks panel beneath it.
        select(null)
        useMilestones.getState().reveal(id)
      }}
      runs={runs}
      roles={roles}
      roleColors={roleColors}
      assigneeHint={hint}
      onOpenRun={(run) => guarded(openRun(run))}
      onPauseRun={(run) => guarded(pauseRun(run))}
      onResumeRun={(run) => guarded(resumeRun(run))}
      nowMs={nowMs}
      editing={edit}
      onEditing={setEditing}
      onClose={() => select(null)}
      /*
       * The session ref is merged in here rather than stored in `specCard`, so it follows the
       * workspace tree and the awaiting set instead of the last four-subprocess read. See
       * `sessionPane` above.
       */
      spec={
        change === null
          ? undefined
          : specCard === null
            ? null
            : { ...specCard, session: sessionRef }
      }
      onOpenSession={(id, mode) => guarded(openSession(id, mode))}
      specProblem={specProblem}
      specEdit={
        change === null || specCard === null
          ? undefined
          : {
              target: editTarget,
              draft: editDraft,
              busy: editBusy,
              problem: editProblem,
              onDraft: setEditDraft,
              onCancel: () => {
                setEditTarget(null)
                setEditDraft(null)
                setEditProblem(null)
              },
              onSave: () => {
                if (project === null || editDraft === null || editTarget === null) return
                const at = parseTarget(editTarget)
                const delta = at === null ? undefined : specCard.deltas[at.delta]
                const requirement =
                  at === null || delta === undefined
                    ? undefined
                    : delta.requirements[at.requirement]
                if (delta === undefined || requirement === undefined) return
                setEditBusy(true)
                setEditProblem(null)
                guarded(
                  specApi
                    .setRequirement({
                      project,
                      change: change as never,
                      spec: delta.spec as never,
                      operation: delta.op as never,
                      // The name the block is addressed by is the one it had when the editor
                      // opened, never the one in the draft: renaming a requirement is a RENAMED
                      // delta, and Rust refuses a header that does not match. Sending the new
                      // name would ask it to replace a block that does not exist.
                      requirement: requirement.name,
                      block: compose(editDraft),
                    })
                    .then((outcome) => {
                      setEditBusy(false)
                      if (outcome.kind === 'written') {
                        setEditTarget(null)
                        setEditDraft(null)
                        return
                      }
                      // Both failures leave the form up with the typing in it. A save that closed
                      // the editor and reported elsewhere would throw away the paragraph it
                      // failed to write.
                      setEditProblem(
                        outcome.kind === 'conflicted'
                          ? { kind: 'conflicted', messages: [outcome.path] }
                          : {
                              kind: 'regressed',
                              messages: outcome.issues.map((issue) => issue.message),
                            },
                      )
                    })
                    .catch((error: unknown) => {
                      setEditBusy(false)
                      throw error
                    }),
                )
              },
            }
      }
      onSpecEditOpen={(target) => {
        const at = parseTarget(target)
        const requirement =
          at === null ? undefined : specCard?.deltas[at.delta]?.requirements[at.requirement]
        if (requirement === undefined) return
        setEditTarget(target)
        setEditDraft(draftOf(requirement))
        setEditProblem(null)
      }}
      onOpenSpecFile={(path) => {
        if (project !== null) guarded(file.open(project, path).then(() => undefined))
      }}
      specBusy={specBusy}
      onSpecPrimary={(task, action) => {
        // `approve` is *assigning*, which the assignee row already does and which the trigger in
        // Rust turns into a dispatch — so the button focuses that decision rather than making it
        // for the user. `accept` is the one gesture that belongs here.
        if (action === 'accept' && project !== null) {
          /*
           * The board refreshes itself: `spec_accept` broadcasts both `tasks-changed` and
           * `spec-changed`, and both stores adopt what arrives. Asking again here would be a
           * second read of state that has already been pushed.
           *
           * The flag around it is what the card draws as a spinner. Cleared in every arm and not
           * only on success — a refusal that left the button inert for ever would be a worse
           * failure than the silence this replaced.
           */
          setSpecBusy(true)
          guarded(
            specApi
              .accept(project, task as never)
              .then((outcome) => {
                setSpecBusy(false)
                // What it did, in a notice. `refused` and `conflicts` are `Ok` arms, so
                // without this the two failures reach nothing at all — see `acceptNotice`.
                const said = acceptNotice(outcome)
                notify(said.text, { kind: said.kind, detail: said.detail })
              })
              .catch((error: unknown) => {
                setSpecBusy(false)
                throw error
              }),
          )
        }
      }}
      /*
       * Drawn only when this project can actually run the workflow. (M28)
       *
       * `undefined` is *no button*, which is the whole optionality claim — see the prop's doc.
       * The board being `ready` is not enough on its own: a project can have `openspec/` and no
       * `.claude/skills/openspec-propose/`, and a button that always refuses is worse than one
       * that is not there.
       */
      onProposeChange={
        canPropose
          ? (task) => {
              if (project === null) return
              guarded(
                specApi.proposeForTask(project, task as never).then((session) => {
                  // Closed and revealed, `onDispatchTo`'s rule and its reason: the line has gone
                  // into a pane, and leaving the modal over it hides the thing the press was
                  // about. Chained, so a refusal leaves the card up carrying it.
                  select(null)
                  return openTaskSession(String(session), { project })
                }),
              )
            }
          : undefined
      }
      deleteArmed={armedDelete(board, deleteArmed) === open.id}
      onDeleteArm={(task) => {
        if (board.kind !== 'ready') return
        setDeleteArmed({ task, rev: board.rev })
      }}
      onDelete={(task) => {
        setDeleteArmed(null)
        guarded(removeTask(task))
      }}
      links={links}
      linkTargets={linkTargets}
      linkAdd={linkAdd}
      onLinkAdd={setLinkAdd}
      /* The M30 wire shapes' dispatch sites, landed with the shapes themselves — `setChange`
         shipped without one and sat unreachable from any UI; `TaskEdit`'s own rule is the
         gesture and the shape in the same commit. */
      onLink={(task, kind, target) => guarded(editTask(task, { kind: 'link', link: kind, target }))}
      onUnlink={(task, kind, target) => {
        // Through the guard: a chip whose kind this build cannot read never draws an ✕, so
        // this is belt-and-braces against a caller the card did not make.
        if (isLinkKind(kind)) guarded(editTask(task, { kind: 'unlink', link: kind, target }))
      }}
      /* Navigation: the card is keyed on the open id, so moving the selection remounts it
         cleanly and the `selected` effects clear the drafts. A gone target opens nothing —
         `openTask` finds no row and the card simply closes onto the board, which is the honest
         rendering of "this task is not here". */
      dispatchTargets={change === null ? undefined : targets}
      dispatchOpen={dispatchOpen}
      onDispatchOpen={setDispatchOpen}
      onDispatchTo={(task, target) => {
        if (project === null) return
        setDispatchOpen(false)
        /*
         * **One road each, and that is what stops a double start.**
         *
         * A role is assigned and nothing else happens here: `cide_agents::autodispatch` sees the
         * assignment edge and starts the run through the same queue the Agents panel uses. Doing
         * anything more on this branch — dispatching as well as assigning — is exactly how a
         * task ends up with two runs on it.
         *
         * A conversation is not an assignment edge at all. `spec_dispatch_to_session` writes
         * `Task::session`, which no trigger reads, and types the task in. See `DispatchTarget`.
         */
        if (target.kind === 'role') {
          guarded(editTask(task, { kind: 'assign', agent: target.id as never }))
          return
        }
        if (target.kind === 'session') {
          /*
           * And then show it. Choosing a conversation left the modal standing over the pane it
           * had just typed the task into — the thing the user asked to see, behind a scrim. The
           * reveal is chained rather than fired alongside, so a dispatch that Rust refuses (a
           * pane closed while the picker was open) leaves the card up with the refusal on it
           * instead of navigating away from the error.
           */
          guarded(
            specApi
              .dispatchToSession(project, task as never, target.id as never)
              .then(() => {
                select(null)
                return openTaskSession(target.id, { project })
              }),
          )
          return
        }
        // Fresh: make the pane first, then hand the task to the session it came up with. The
        // pane has to exist before there is a session to name.
        const consoleTab = workspaceProject?.tabs[0]
        if (consoleTab === undefined) return
        guarded(
          addRow(project, consoleTab.id, null, 'after', { kind: 'newClaude' }).then(
            async (created) => {
              /*
               * The pane exists now, but *this* component's copy of the tree does not know it:
               * the snapshot arrives on `workspace_changed`, one round trip later. So the
               * session is read from the store's current state rather than from the closure,
               * which is the value that has been updated by the time this resolves.
               */
              const fresh = useWorkspace
                .getState()
                .boot?.workspace.projects[String(project)]?.tabs[0]
              const paneNode = fresh?.tree.panes[String(created.pane)]
              const session = paneNode?.session ?? null
              if (session === null) return
              await specApi.dispatchToSession(project, task as never, session as never)
              // The pane was made a moment ago and is not on screen unless its tab is the one
              // showing; reveal it for the same reason the branch above does.
              select(null)
              await openTaskSession(String(session), { project })
            },
          ),
        )
      }}
      onOpenTask={(task) => select(task)}
      onSetTitle={(task, title) => guarded(editTask(task, { kind: 'setTitle', title }))}
      onSetStatus={(task, status) => guarded(editTask(task, { kind: 'setStatus', status }))}
      onSetAssignee={(task, agent) => guarded(editTask(task, { kind: 'assign', agent }))}
      onSetBody={(task, body) => guarded(editTask(task, { kind: 'setBody', body }))}
      onAddComment={(task, text, attachments) =>
        // Text alone is a `TaskEdit::Comment`; text with files is one `task_attach` on the
        // `newComment` target, so the comment and its screenshots land together or not at all.
        guarded(
          attachments.length === 0
            ? editTask(task, { kind: 'comment', text })
            : attachTask(task, { kind: 'newComment', text }, attachments),
        )
      }
      /* The author is not sent and cannot be: `task_edit` passes `TaskAuthor::User`, decided
         by the command rather than by this payload, which is what makes the Rust guard
         unforgeable rather than merely checked. See `TaskComment`. */
      onEditComment={(task, comment, text) =>
        guarded(editTask(task, { kind: 'editComment', id: comment, text }))
      }
      onDeleteComment={(task, comment) =>
        guarded(editTask(task, { kind: 'deleteComment', id: comment }))
      }
      /* Attachments (M39). The picker and the clipboard are promises the card awaits; every
         mutation goes through `guarded` like the rest; the viewer is this host's state. */
      previews={previews}
      onPickAttachments={pickAttachments}
      onStageClipboard={stageClipboard}
      onAttach={(task, target, sources) => guarded(attachTask(task, target, sources))}
      onAttachClipboard={(task, target) =>
        guarded(
          attachClipboard(task, target).then((attached) => {
            if (!attached) notify('The clipboard holds no image.', { kind: 'warn' })
          }),
        )
      }
      onDetachAttachment={(task, attachment) =>
        guarded(
          editTask(task, { kind: 'detachAttachment', attachment }).then(() =>
            forgetPreview(attachment),
          ),
        )
      }
      onOpenAttachment={(task, attachment) => {
        if (project !== null) guarded(attachmentsApi.open(project, task as never, attachment))
      }}
      onRevealAttachment={(task, attachment) => {
        if (project !== null) guarded(attachmentsApi.reveal(project, task as never, attachment))
      }}
      onViewAttachment={(_task, attachment) => setLightbox(attachment)}
      composerStaged={composerStaged}
      onComposerStaged={setComposerStaged}
      dropHot={dropHot}
    />
    {lightbox !== null && (
      <AttachmentLightbox
        images={imageAttachmentsOf(detail)}
        current={lightbox}
        previews={previews}
        onClose={() => setLightbox(null)}
        onStep={setLightbox}
        onOpen={(attachment) => {
          if (project !== null) guarded(attachmentsApi.open(project, open.id as never, attachment))
        }}
      />
    )}
    {/* Verify's whole log for this task, over the card (M83). */}
    {verifyLog && project !== null && (
      <CheckLogModal
        project={project}
        target={{ kind: 'verify', key: String(open.id), title: `Verify of ${String(open.id)}` }}
        running={verify?.running === true}
        onClose={() => setVerifyLog(false)}
      />
    )}
    </>
  )
}

/*
 * The non-`ready` fallback for `runs`, module-level for the reason `TasksPanelHost`'s copy is:
 * a fresh `[]` per render would be a new prop identity for a value that has not changed.
 */
const NO_RUNS: readonly RunRef[] = []
const NO_LINKS: readonly LinkChip[] = []
const NO_TARGETS: readonly LinkTargetOption[] = []
