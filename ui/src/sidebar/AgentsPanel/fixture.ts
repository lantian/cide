/**
 * The Agents panel's named states, as plain props. (M18)
 *
 * # Why these exist
 *
 * Most of them are states nobody can arrange on demand while building the panel. A run that
 * was frozen long enough for its model request to time out, a role whose harness is not
 * installed, a phase string from a cide that is newer than this one — each is a screen that
 * has to be right the first time it appears, and the first time it appears must not also be
 * the first time anybody has looked at it.
 *
 * `ui/scripts/check-agents-render.mjs` renders every story here through `react-dom/server` and
 * asserts on the markup. Three of the assertions are the reason the fixtures are shaped the way
 * they are: `roles-resting` must render **no** Open element at all (not a disabled one) *and*
 * a Configure in every role row, which is only a real assertion because this file hands that
 * story a live `onOpen` beside the `onConfigure`; `role-queued` must do the same over a role
 * that has a run — because a queued run has no session, which is a different reason for the
 * same absence; and `roster-unknown` must render no button and none of the designed sentences,
 * which is only a real assertion because it hands that story every handler as well.
 *
 * # The stories are named for a **role's** state, because that is what a row is now
 *
 * The panel is a list of subagents, so the states worth pinning are a role's: resting, running,
 * idle, queued, paused, awaiting permission, unavailable, several runs at once, and a role the
 * roster no longer defines at all. Each is one story, and the run states they are built from are
 * the same `RunView`s as before.
 *
 * # Milliseconds are `number`
 *
 * `AgentRun::startedUnixMs` is a `u64` and therefore a `bigint` on the wire, and `adapt.ts`
 * converts it. Everything here is a plain `number`, matching `model.ts`: mixing a `bigint` with
 * a `number` in a comparison throws a `TypeError` rather than coercing, `elapsed()` does exactly
 * that arithmetic inside a render, and React 19 unmounts the tree on a render that throws — so
 * the failure of getting this wrong is a blank window, not a wrong number.
 */
import type { ProjectId } from '@/ipc/client'
import type { AgentsPanelViewProps } from './AgentsPanel'
import type { AgentDefView, Roster, RunPhase, RunView } from './model'

/**
 * A fixed instant, so a digest of a story is the same on two runs.
 *
 * 2025-11-24T17:20:00Z, and the only thing that matters about it is that it does not move.
 */
export const NOW_MS = 1_764_005_000_000

/** A project id, which is a branded string the view only ever compares against `null`. */
export const PROJECT = 'ffffffff-0000-4000-8000-00000000cafe' as unknown as ProjectId

/** The path the disabled screen prints in full, and that the render check greps for. */
export const CONFIG_PATH = '/home/dev/work/thing/.cide/config.json'

const TASK_TITLES: Readonly<Record<string, string>> = {
  't-14': 'Add the retry bar',
  't-15': 'Sweep the phase table',
}

function role(over: Partial<AgentDefView> & Pick<AgentDefView, 'id' | 'label'>): AgentDefView {
  return {
    scope: 'project',
    harness: 'claude',
    description: 'Implements one task end to end and reports back on it.',
    systemPrompt: 'You are the developer agent for this project. …',
    model: 'sonnet',
    // No `color:` in the definition, which is the ordinary case and the one worth being the
    // default: every story below therefore exercises the *derived* colour, and the one story
    // that declares one says so on its own line.
    color: null,
    unavailable: null,
    maxConcurrent: 1,
    // The wire's own default. A role that isolates is the ordinary role, and a story that had
    // to say so on every line would bury the one story where it matters.
    worktree: true,
    ...over,
  }
}

const DEVELOPER = role({ id: 'developer', label: 'Developer' })
const QA = role({
  id: 'qa',
  label: 'QA',
  description: 'Reviews a finished task against what it claimed to do.',
  maxConcurrent: 2,
})
/*
 * A role cide can see and cannot run.
 *
 * `unavailable` rather than absent, deliberately: "we could not tell" and "it is not there"
 * must stay distinguishable, so an uninstalled harness greys the role and keeps its sentence
 * instead of quietly dropping it from the list.
 */
const ARTIST = role({
  id: 'artist',
  label: 'Artist',
  harness: 'opencode',
  description: 'Produces the assets a task asks for.',
  unavailable: 'opencode is not installed.',
})

/*
 * The two roles cide did not author. (M30)
 *
 * A subagent read out of `.claude/agents/` behaves like every other row — it dispatches, it takes
 * a slot, it has a description — and the *only* thing that distinguishes it is where it came
 * from. So both fixtures are deliberately ordinary in every other respect: if a badged row
 * rendered differently anywhere else, the render check would catch it as a diff against these.
 *
 * Both scopes, because they must draw the **same** mark and only a pair can assert that.
 */
const REVIEWER = role({
  id: 'code-reviewer',
  label: 'Code Reviewer',
  scope: 'claudeProject',
  description: 'Reviews a diff for correctness and says what it would change.',
})
const RESEARCHER = role({
  id: 'researcher',
  label: 'Researcher',
  scope: 'claudeGlobal',
  description: 'Reads around a question and reports what it found.',
})

/**
 * The phases in which a run's worked clock is running — `RunState::counts_as_work`'s answer,
 * restated here because this module imports nothing but `model.ts`.
 *
 * The fixtures respect the invariant rather than setting the two clock fields freely: a story
 * carrying `workingSinceMs` on a paused row would be asserting on a row no registry can
 * produce, and the render check would then be pinning a rendering of an impossible state.
 */
const WORKING_PHASES: readonly RunView['phase'][] = ['starting', 'running']

function run(over: Partial<RunView> & Pick<RunView, 'run' | 'agent' | 'agentLabel'>): RunView {
  const phase = over.phase ?? 'running'
  const startedMs = over.startedMs ?? NOW_MS - 125_000
  // The default is a run that has worked every millisecond since it was dispatched, which is
  // both a real state and the one that leaves each existing story's figure where it was. The
  // rows where the two clocks *diverge* say so explicitly below — that divergence is what this
  // panel now draws, so it has to be in the corpus rather than implied by a default.
  const working = WORKING_PHASES.includes(phase)
  return {
    harness: 'claude',
    session: null,
    phase: 'running',
    task: null,
    startedMs: NOW_MS - 125_000,
    workedMs: working ? 0 : Math.max(0, NOW_MS - startedMs),
    workingSinceMs: working ? startedMs : null,
    pausedSinceMs: null,
    exitCode: null,
    failure: null,
    staleTurn: false,
    note: null,
    // What Rust computes from the session and the conversation: a row with a session is
    // openable, which keeps every story's Open control where it was.
    openable: (over.session ?? null) !== null,
    ...over,
  }
}

const RUNNING = run({
  run: 'r-0001',
  agent: 'developer',
  agentLabel: 'Developer',
  session: 's-0001',
  phase: 'running',
  task: 't-14',
})

/*
 * A queued run, and the two things about it the panel has to get right: it has **no session**,
 * so there is nothing to attach a pane to and the row must draw no Open element; and it names
 * a task, so the agent→task link is still there to click.
 */
const QUEUED = run({
  run: 'r-0002',
  agent: 'qa',
  agentLabel: 'QA',
  phase: 'queued',
  task: 't-15',
  startedMs: NOW_MS - 8_000,
  // Eight seconds old and none of it work: the queue is a wait, and the old figure counted it.
  workedMs: 0,
  note: 'Waiting for a free worktree.',
})

/**
 * The same queued run, carrying the note `AgentRegistry::runs_for` derives for a **paused**
 * project — the exact string `crates/cide-app/src/agents.rs`'s `PAUSED_QUEUE_NOTE` holds.
 *
 * The note is the whole mechanism: Rust decides the reason a run is held and the row simply
 * renders it, so a paused project needs no new phase, no new DTO field and no view logic. That is
 * also what makes this fixture worth having — nothing in the frontend would fail if the reason
 * stopped arriving, and this story is the only place that would notice.
 */
const QUEUED_BY_PAUSE = run({
  run: 'r-0002',
  agent: 'qa',
  agentLabel: 'QA',
  phase: 'queued',
  task: 't-15',
  startedMs: NOW_MS - 8_000,
  workedMs: 0,
  note: "this project's agents are paused; nothing starts until Resume",
})

const PAUSED = run({
  run: 'r-0003',
  agent: 'developer',
  agentLabel: 'Developer',
  session: 's-0003',
  phase: 'paused',
  task: 't-14',
  startedMs: NOW_MS - 3_845_000,
  // **The row this change exists for.** An hour and four minutes since dispatch, forty-nine of
  // them worked and fifteen frozen — so the figure drawn is `49m` where it used to be `1h 04m`,
  // and a fixture where the two agreed could not see that. The gap is `pausedSinceMs`'s, which
  // until now was carried to the view and read by nothing.
  workedMs: 2_945_000,
  pausedSinceMs: NOW_MS - 900_000,
})

/* A run with no task at all — the row that draws `no task` rather than a blank line. */
const ADRIFT = run({
  run: 'r-0004',
  agent: 'qa',
  agentLabel: 'QA',
  session: 's-0004',
  phase: 'awaitingPermission',
  startedMs: NOW_MS - 61_000,
})

/*
 * An idle run: the turn is handed back and **the child is alive**.
 *
 * The case `canOpen` most exists for — a complete transcript in a pane the user can then type
 * the next turn into — so this is the story that proves Open is offered for a role that is not
 * computing anything. It is not the same as `finished`, and the panel must not draw it as one.
 */
const IDLE = run({
  run: 'r-0010',
  agent: 'developer',
  agentLabel: 'Developer',
  session: 's-0010',
  phase: 'idle',
  task: 't-14',
  startedMs: NOW_MS - 240_000,
})

/*
 * A second run of QA, so `role-multi-run` has two lines under one role.
 *
 * This used to be reachable only with `isolation: shared`; per-task worktrees made it the
 * ordinary shape of a fanned-out role (each task in its own checkout, up to the role's
 * `max-concurrent`), so the fixture went from guarding a configuration almost nobody has to
 * pinning one everybody with two assigned tasks will see. It is `running` beside `ADRIFT`'s
 * `awaitingPermission`, so the role's one-word summary has to choose, and the choice is the
 * assertion.
 */
const QA_RUNNING = run({
  run: 'r-0011',
  agent: 'qa',
  agentLabel: 'QA',
  session: 's-0011',
  phase: 'running',
  task: 't-15',
  startedMs: NOW_MS - 600_000,
})

/*
 * A live run of a role the roster does not define — somebody deleted `.cide/agents/ghost.md`
 * while it was working.
 *
 * `sections()` invents a row for it rather than dropping it, and that is the point of the
 * story: a run with no row is a `claude` spending the user's quota that they cannot see, cannot
 * open and cannot stop.
 */
const GHOST = run({
  run: 'r-0012',
  agent: 'ghost',
  agentLabel: 'Ghost',
  session: 's-0012',
  phase: 'running',
  task: 't-14',
  startedMs: NOW_MS - 45_000,
})

const FINISHED = run({
  run: 'r-0005',
  agent: 'qa',
  agentLabel: 'QA',
  session: 's-0005',
  phase: 'finished',
  task: 't-15',
  startedMs: NOW_MS - 7_400_000,
  // Dispatched two hours ago, ran for six minutes, has been over ever since. A History row's
  // figure is **final**, and this one is two hours short of what the old one drew — which rose
  // by another hour every hour the panel stayed open.
  workedMs: 372_000,
  exitCode: 0,
})

const FAILED = run({
  run: 'r-0006',
  agent: 'artist',
  agentLabel: 'Artist',
  harness: 'opencode',
  phase: 'failed',
  startedMs: NOW_MS - 9_000_000,
  failure: 'opencode exited before its first turn.',
})

/*
 * A phase from a cide newer than this one, and a phase that is a prototype key.
 *
 * Cast rather than written as a `RunPhase`, because that is precisely the situation: the wire
 * type says `RunPhase` and the producer says otherwise. `'constructor'` is the member that an
 * `?? fallback` does not catch — a table lookup returns `Object.prototype.constructor`, a
 * function, which React refuses as a child and which `className` stringifies into the whole
 * source text of `Object`.
 */
const ROGUE = run({
  run: 'r-0007',
  agent: 'developer',
  agentLabel: 'Developer',
  session: 's-0007',
  phase: 'constructor' as unknown as RunPhase,
  task: 't-14',
  startedMs: NOW_MS - 30_000,
})

const STALE = run({
  run: 'r-0008',
  agent: 'developer',
  agentLabel: 'Developer',
  session: 's-0008',
  phase: 'paused',
  task: 't-14',
  startedMs: NOW_MS - 2_000_000,
  pausedSinceMs: NOW_MS - 1_800_000,
  staleTurn: true,
})

/* A second frozen run, so the full-freeze story is a freeze of more than one thing. */
const PAUSED_QA = run({
  run: 'r-0009',
  agent: 'qa',
  agentLabel: 'QA',
  session: 's-0009',
  phase: 'paused',
  task: 't-15',
  startedMs: NOW_MS - 400_000,
  pausedSinceMs: NOW_MS - 120_000,
})

function ready(runs: readonly RunView[], agents: readonly AgentDefView[] = [DEVELOPER, QA]): Roster {
  return { kind: 'ready', agents, runs, dispatching: true }
}

/**
 * The same roster with the **dispatch queue shut** — which is what, and only what, a
 * project-scope pause does to it.
 *
 * `AgentRegistry::dispatching` reads one thing, `paused_projects`, and a per-run pause never
 * touches it; so `dispatching: false` means a `agents_pause(project, None)` happened, which also
 * froze this project's own console session. That session has no row in this panel and never
 * will — it is not a run — so **in these stories the strongest evidence on screen that the user
 * is frozen is this flag**, and in [`project-paused-no-live-run`] it is the only evidence.
 */
function queueShut(
  runs: readonly RunView[],
  agents: readonly AgentDefView[] = [DEVELOPER, QA],
): Roster {
  return { kind: 'ready', agents, runs, dispatching: false }
}

/**
 * Every handler wired to a no-op.
 *
 * Handed to **every** story, including the ones that must render nothing: a check that asserts
 * "no Open button appears" against a story that was given no `onOpen` asserts nothing at all.
 * The gates being tested are the model's — `canOpen`, `canPause`, `canDispatch`, the roster's
 * own arm — and they can only be seen doing their work when the alternative was available.
 */
const HANDLERS = {
  onDispatch: () => {},
  onOpen: () => {},
  onPause: () => {},
  onResume: () => {},
  onStop: () => {},
  onRevealTask: () => {},
  onRetryTurn: () => {},
  onAckStaleTurn: () => {},
  onEnable: () => {},
  onRevealConfig: () => {},
  /*
   * Integrate, and its arming half. Handed to every story for the reason the rest of this map
   * is: the assertion that matters is that an *unarmed* role shows Integrate and not Merge, and
   * a story given no handler could not have shown either, so it would pass for the wrong reason.
   */
  onIntegrateArm: () => {},
  onIntegrate: () => {},
  /*
   * The project scope. Handed to **every** story for the reason the whole map is: the
   * assertions that matter about this pair are the negative ones — no Resume in the header of a
   * project with nothing frozen — and a story that was given no `onResumeAll` could not have
   * drawn one whatever the gate decided.
   */
  onPauseAll: () => {},
  onResumeAll: () => {},
  /*
   * Configure, and the empty screen's screen-level twin. Handed to **every** story for the
   * reason the whole map is: the assertion that matters is that Configure is in *every* role row
   * whatever the role's state — including a role whose harness is not installed and a role the
   * roster does not define — and a story that had not been given the handler could not have
   * drawn one whatever the view decided.
   */
  onConfigure: () => {},
  onConfigureAll: () => {},
} as const

function story(over: Partial<AgentsPanelViewProps>): AgentsPanelViewProps {
  return { project: PROJECT, nowMs: NOW_MS, taskTitles: TASK_TITLES, ...HANDLERS, ...over }
}

export type AgentsStoryName =
  | 'no-project'
  | 'roster-unknown'
  | 'disabled'
  | 'empty'
  | 'roles-resting'
  | 'role-running'
  | 'role-idle'
  | 'role-queued'
  | 'project-paused-queued-run'
  | 'role-paused'
  | 'role-awaiting'
  | 'role-finished-only'
  | 'role-multi-run'
  | 'role-unavailable'
  | 'role-claude-code'
  | 'role-declared-colour'
  | 'role-undefined'
  | 'stale-turn'
  | 'rogue-phase'
  | 'project-paused'
  | 'project-paused-no-live-run'
  | 'integrate-unarmed'
  | 'integrate-armed'

export const AGENTS_STORIES: Record<AgentsStoryName, AgentsPanelViewProps> = {
  'no-project': story({ project: null, roster: ready([RUNNING]) }),

  /*
   * Nobody has looked yet. The whole assertion is negative — no button, none of the designed
   * sentences — because the alternative, drawing the `disabled` screen for the frame before
   * the answer lands, tells a user whose project has subagents on that they are off, directly
   * under a button that writes a committed file into their repository.
   */
  'roster-unknown': story({ roster: { kind: 'unknown' } }),

  disabled: story({
    roster: {
      kind: 'disabled',
      hint: 'No .cide/config.json in this project, so subagents are off.',
      configPath: CONFIG_PATH,
    },
  }),

  /*
   * On, and no roles at all: **the centred button into Settings**, and nothing else but the
   * claim, one sentence and the quiet reveal. What used to be here was a worked example of a
   * role file — the panel answering "you have no subagents" with a page of YAML to copy — and
   * the render check greps for its absence by name.
   */
  empty: story({ roster: { kind: 'empty', configPath: CONFIG_PATH } }),

  /*
   * **The user's rule, in its plainest form.** Two roles, neither doing anything: no Open
   * element anywhere in the document, and a Configure in both rows. It is a real assertion
   * because `HANDLERS` gives this story a live `onOpen` — the gate is what withholds the
   * button, not a missing handler.
   */
  'roles-resting': story({ roster: ready([]) }),

  'role-running': story({ roster: ready([RUNNING]) }),

  /*
   * A role whose turn is over and whose child is alive. Open is offered — this is the case
   * `canOpen` most exists for — and the status must read `Idle` rather than `Finished`: nothing
   * exited, and the row can still be given another turn.
   */
  'role-idle': story({ roster: ready([IDLE]) }),

  /*
   * A role with a run and **still no Open**, for a different reason than `roles-resting`: the
   * run is queued, so there is no session to mirror. The pair is what makes the assertion mean
   * something — one absence is "nothing is happening", the other is "something is, and it has
   * no screen yet" — and neither draws a disabled button.
   */
  'role-queued': story({ roster: ready([QUEUED]) }),

  'role-paused': story({ roster: ready([PAUSED]) }),

  /* Blocked on the user, and with no task — so the row also draws `no task`. */
  'role-awaiting': story({ roster: ready([ADRIFT]) }),

  /*
   * Both roles' runs have ended. Every role row is back to resting with no Open on it, and the
   * two runs are in Recent — which is the only place their transcripts are reachable from, and
   * the reason that section survived the rewrite.
   */
  'role-finished-only': story({ roster: ready([FINISHED, FAILED]) }),

  /*
   * **Two runs under one role**, which needs `isolation: shared` to happen at all. Both lines are
   * drawn — a run on screen nowhere is a `claude` nobody can stop — and the role's one-word
   * summary is the *more demanding* of the two, not the older: `awaitingPermission` is blocked on
   * the user, and a summary that read `Running` would hide the one line where a human is the
   * bottleneck.
   */
  'role-multi-run': story({ roster: ready([QA_RUNNING, ADRIFT]) }),

  /*
   * Three roles: one dispatchable, one at its ceiling because a run of it is live, and one
   * whose harness is missing. Every row must carry exactly one of a Dispatch button and a
   * sentence — and **all three must carry Configure**, the unavailable one included, because a
   * role that cannot run is the one somebody most wants to open the settings for.
   */
  'role-unavailable': story({
    roster: ready([RUNNING], [DEVELOPER, QA, ARTIST]),
  }),

  /*
   * Claude Code subagents beside a cide role. (M30)
   *
   * The whole assertion is the badge: **two** rows carry it, they carry the *same* text, and the
   * cide role beside them carries none. A story with only one badged row could not tell a badge
   * keyed on the scope from one keyed on anything else, and a story with no unbadged row could
   * not tell a badge from a chip drawn on every row.
   */
  'role-claude-code': story({
    roster: ready([], [DEVELOPER, REVIEWER, RESEARCHER]),
  }),

  /*
   * A live run of a role the roster no longer defines — the file was deleted while it worked.
   *
   * The row is invented from the run itself, so the agent stays visible, openable and
   * stoppable; it is refused a dispatch with a sentence of its own, because there is no
   * definition to dispatch from.
   */
  /*
   * A role whose definition asked for a colour, beside one that did not. (M75)
   *
   * The whole story is the pair: `developer` derives its hue from a hash of its id — what every
   * role with no `color:` gets, and what every *other* story in this file therefore already
   * exercises — while `designer` declares `green`, which is deliberately not the hue its own
   * id would derive to (`cyan`). Without the second half, a build that silently ignored `AgentDef.color`
   * would pass every assertion in this file, because the derived colour is right either way.
   */
  'role-declared-colour': story({
    roster: ready([RUNNING], [DEVELOPER, { ...QA, id: 'designer', label: 'Designer', color: 'green' }]),
  }),

  'role-undefined': story({ roster: ready([RUNNING, GHOST]) }),

  /*
   * The stale-turn bar, on the run it is about, under the role that owns it. A second role with
   * a run follows so the check can slice the bar out of the document by its own element and
   * count the buttons inside it exactly.
   */
  'stale-turn': story({ roster: ready([STALE, ADRIFT]) }),

  /*
   * A project-scope pause, mid-flight: the queue is shut and both runs are frozen — and so, off
   * screen, is the console session the user types into. The header must offer Resume, and every
   * role must refuse a dispatch with `canDispatch`'s sentence rather than a live button.
   */
  'project-paused': story({ roster: queueShut([PAUSED_QA, PAUSED]) }),

  /*
   * **A paused project with work waiting in it** — the state a terrastrike probe sat in for hours
   * while every surface cide has called it an ordinary queue.
   *
   * `queueShut` plus a `queued` run, which no story combined before: `project-paused` pauses runs
   * that had already started, and `project-paused-no-live-run` has nothing in it at all. Neither
   * covers the case the audit found, where the queue is shut and something is waiting behind it.
   *
   * What the row must say is *why* it is waiting, and it says it through `note` alone.
   */
  'project-paused-queued-run': story({ roster: queueShut([QUEUED_BY_PAUSE]) }),

  /*
   * **The story the Resume control exists for, and the one a refactor will break.**
   *
   * The same project-scope pause, over runs that have all ended. Nothing on screen reads
   * `paused`; nothing reads `running`; `metaFigure` says `0`; every role row is resting and every
   * run is in Recent, which is closed by default. The user's console pane is frozen all the same
   * — `agents_pause` freezes the primary session whether or not there was a single run to freeze
   * beside it — and the one fact that says so is `dispatching: false`.
   *
   * So Resume must still be in the header. Any offer condition that reads the rows, the live
   * count or "is the roster busy" is false here and leaves a window with no way out of a state
   * cide put it in. `crates/cide-app/src/agents.rs` names this exact case: "the header has to be
   * able to draw Resume for a project whose runs are all finished".
   */
  'project-paused-no-live-run': story({ roster: queueShut([FINISHED, FAILED]) }),

  /*
   * A phase this build has never heard of, beside one it has.
   *
   * Two runs on purpose, and now under two different roles. The regression being pinned is
   * `check-problems.mjs`'s: an unrecognised value used to zero the header count and blank the
   * list. Here it would take a different route to the same place — a membership test over
   * `ACTIVE_PHASES` would file the run as neither active nor done and it would appear under no
   * role and in no Recent — so the assertion is that **both rows are still on screen** and the
   * header still reads `1`.
   */
  'rogue-phase': story({ roster: ready([ADRIFT, ROGUE]) }),

  /*
   * Integrate, unarmed — every role offers it, whatever its runs are doing. A role's branch
   * outlives its runs, so this is not a run action and is not gated on one; `upToDate` is the
   * honest answer when the branch holds nothing, and the panel cannot know that without asking.
   */
  'integrate-unarmed': story({ roster: ready([]) }),

  /*
   * Integrate, armed — the confirm-on-second-click state, which changes the user's *own* branch
   * and so is the one gesture in this panel that earns a second press.
   */
  'integrate-armed': story({ roster: ready([]), integrateArmed: 'developer' }),
}
