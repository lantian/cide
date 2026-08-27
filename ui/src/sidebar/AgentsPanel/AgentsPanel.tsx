/**
 * The ◍ Agents sidebar panel: **one row per subagent, saying what it is doing right now** — and,
 * folded away underneath, what has already finished. (M18)
 *
 * # What this panel is a list of
 *
 * Roles. Not runs. It used to be five groups — Running, Idle, Queued, Roles, Recent — and the
 * user who ran it said what was wrong with that in one sentence: they wanted their subagents and
 * each one's status, and instead a role with nothing running appeared only under *Roles*, a role
 * with a run appeared twice, and answering "what is `qa` doing" meant reading four lists and
 * joining them by name. `model.ts`'s `SectionKind` carries the whole argument.
 *
 * So a row is a role. It leads with the role's name and a one-word status derived from its runs;
 * under it, one line per run that is actually active, each with its own Open/pause/stop; and at
 * the foot of it the role-scope controls — Dispatch, Integrate, **Configure**.
 *
 * # Open is drawn only when there is something to look at, and Configure always
 *
 * That is the user's own rule and it is enforced in two places that cannot disagree, because the
 * second is derived from the first: `RoleRow.canOpen` is `runs.some(canOpen)`, and the Open
 * buttons themselves are drawn by `ActivityRow`, one per active run, from the same `canOpen`.
 * With nothing active a role has no activity lines at all, so there is no Open element anywhere
 * in the row — **withheld, not disabled**, which is this panel's standing rule: a greyed control
 * promises that some reachable condition would make it work, and here there is none. The control
 * that would change it is Dispatch, on the same row.
 *
 * Configure is on **every** role row including an unavailable one, because a role whose harness
 * is missing is precisely the role somebody wants to open the settings for.
 *
 * # A pure view
 *
 * It reads no store, calls no IPC and does not read the clock. Everything arrives by prop,
 * **including `nowMs`** — a component that called `Date.now()` could not be digested by an SSR
 * check, because the same fixture would render differently on two runs and the digest would
 * assert nothing. That is the same reason `model.elapsed` takes the instant as a parameter.
 * The stateful half is a later slice's `AgentsPanelHost`, and `index.ts` is where it will be
 * exported from; nothing in this file has to change when it arrives.
 *
 * # Four roster states, and the fourth is the one that is easy to drop
 *
 * `unknown` — *nobody has looked yet* — draws **nothing**. Not the disabled prose, not an
 * empty list, not a spinner (this codebase has no spinner for a sub-second command, and a
 * flash of one is its own artefact). `agents.roster` reaches the store through
 * `pendingCommand` with a `null` fallback, so there is a real interval on every open of this
 * panel — and the whole of it on a build without the handler — in which the answer is not in
 * yet. Drawing `disabled`'s screen during that interval would tell a user whose project has
 * subagents *on* that they are off, one frame before the truth arrives, directly under a
 * button that writes a committed file into their repository. `ui/scripts/check-agents-render.mjs`
 * pins it from the other end: the `roster-unknown` story must render no button at all and none
 * of the designed sentences.
 *
 * # The disabled state is the first thing most users see, so it is designed
 *
 * Subagents are off by default and enabled per project, so this is the screen. It states, in
 * this order: what a subagent is; that turning them on **writes a file the repository will
 * contain**, with the path in full; and only then the button. A feature toggle that quietly
 * adds a tracked file is a surprise commit.
 *
 * The `empty` state — on, but no roles defined — is now a **centred button into Settings**. It
 * used to print a worked example of a role file, and that was the panel telling a user whose
 * project has no agents to go and write some YAML; the screen that answers "I have no subagents"
 * is the one that takes you to where subagents are made. There is still deliberately **no
 * "create three roles for me" button**: inventing a `qa` role the user did not ask for is the
 * same class of lie as a confident empty list, and Settings is where they write their own.
 *
 * `Reveal .cide/` survives beside it, quietly and second. Roles are committed files, the Settings
 * screen writes into that directory, and a user who wants to see what is about to be in their
 * next commit — or to hand-write a definition — has one gesture to get there. It is a secondary
 * control because it is the second thing anybody wants; it is still drawn because it is the only
 * route from this panel to the files themselves.
 *
 * # Never both, never neither
 *
 * A role row carries the Dispatch button **or** the sentence saying why it cannot be
 * dispatched, and both come out of the single `canDispatch` call `sections()` already made.
 * Deriving them from one value is what makes "a greyed control with no explanation" impossible
 * to write here rather than merely discouraged.
 *
 * # The header carries the project scope, and its resume half is the one to be careful with
 *
 * `onPauseAll` / `onResumeAll` freeze and thaw the **whole project** — every run, the dispatch
 * queue, and the project's own console session, which is the pane the user types into. That
 * last part is why the control is in the header at all rather than only on the rows: resume has
 * to be reachable from something that is not the frozen pane.
 *
 * [`ScopeControl`] draws at most one of the two and argues out the offer conditions, which are
 * deliberately not each other's mirror. The short version, because it is the thing a tidy-up
 * will break: **Resume is offered whenever anything is frozen and on no other condition** —
 * including when every run on screen reads `finished` and the only evidence is
 * `dispatching === false`.
 *
 * # A role row can take the role's work back, and it asks twice
 *
 * `Integrate` merges `cide/<role>` into the branch the user has checked out. The whole reason
 * this feature gives every agent a worktree is that the person reviews what the agent did
 * *before* it lands in their branch — and until this control existed only a model could take
 * it, through the `cide_agent_integrate` MCP tool. That was the wrong way round.
 *
 * [`IntegrateControl`] is the second instance of [`ScopeControl`]'s shape — a pair of handlers,
 * at most one button drawn, the conditions argued in one function — and it makes two decisions
 * that are written out there in full: it is offered on **every** role, including ones that
 * cannot be dispatched, because nothing on the wire says whether a role's branch has anything
 * on it; and the merge takes **two clicks**, because it writes the user's working tree.
 */
import {
  OFF_FOR_THIS_PROJECT,
  RECENT_CAP,
  ROSTER_UNKNOWN,
  isDonePhase,
  metaFigure,
  scopeBadge,
  sections,
  type RoleRow,
  type Roster,
  type Section,
} from './model'
import { ActivityRow, RunRow, TONE_CLASS, cx } from './RunRow'
import type { ProjectId } from '@/ipc/client'
import { Icon, asIcon } from '@/icons/Icon'

import styles from './AgentsPanel.module.css'

/**
 * What a role row's **Configure** says it will do.
 *
 * It names the destination, because the button leaves the panel: a control that silently swaps
 * the tab under the user is one they press once. The screen it opens is the Settings tab's
 * Agents section — `SettingsSection::Agents` on the wire — which is where a role's file is
 * written, so this is the one control in the panel that leads somewhere a role can be *changed*
 * rather than merely started or stopped.
 */
const CONFIGURE_TITLE = 'Configure this subagent in Settings'

/**
 * The empty screen's one control, and the whole of that screen.
 *
 * Says *subagents* rather than *roles*: this is read by somebody who has just turned the feature
 * on and has never seen a role file, and the word that means anything to them is the one they
 * saw on the switch. See the header for why the worked YAML example that used to be here is
 * gone.
 */
const CONFIGURE_ALL_LABEL = 'Configure subagents in Settings'


/**
 * What the header's Pause says it will do — **including the part a user would not guess.**
 *
 * A project-scope pause freezes every run *and this project's own console session*: the pane
 * the user types into stops answering the keyboard until they resume. That is `agents_pause`'s
 * design and not an accident (a pause that left the orchestrator running would spend the whole
 * freeze dispatching into a shut queue), but it is not a consequence anybody infers from the
 * word "pause", and discovering it by pressing the button is the worst possible way to learn
 * it. So the sentence names the console before the click rather than after it.
 */
const PAUSE_ALL_TITLE =
  "Pause every subagent, and this project's own Claude session, until you resume"

/**
 * What the header's Resume says. Two clauses, because a pause did two things.
 *
 * `AgentRoster::Ready::dispatching` is on the wire precisely so this control can tell them
 * apart: a project with the queue shut and nothing running is indistinguishable from an idle
 * one, and the thawing of frozen children and the reopening of the queue are separate facts.
 */
const RESUME_ALL_TITLE = 'Resume: continue every frozen agent and reopen the dispatch queue'

/**
 * What the unarmed `Integrate` says. Three clauses, and the third is the one that stops this
 * reading as a destructive button.
 *
 * It names the act (a merge), the destination (*your* branch, not some abstraction), and the
 * fact that the click does not perform it. A control whose first press is harmless has to say
 * so, or a cautious user never presses it and never finds out what it does.
 */
const INTEGRATE_TITLE =
  'Merge this role’s branch into the branch you have checked out. Asks to confirm first.'

/**
 * What the armed one says — and it is the sentence that has to be *true*.
 *
 * The promise it makes is `cide_git::worktree::integrate`'s documented behaviour, not a
 * reassurance: the merge is computed in memory and `Index::has_conflicts` is asked before a
 * single byte is written, so a conflict changes nothing whatsoever and answers with the paths
 * instead. Saying so here is what makes the second click a decision rather than a gamble — and
 * it is why nothing in this feature may ever "help" by leaving a half-merged tree behind.
 */
const INTEGRATE_CONFIRM_TITLE =
  'Merge it now. If it conflicts, nothing is changed at all and you get the list of paths.'

export interface AgentsPanelViewProps {
  /**
   * The active project, or `null` before one is open.
   *
   * Rendered rather than merely carried: with no project open, "subagents are off for this
   * project" names a project that does not exist, which is a more confusing answer than the
   * true one.
   */
  project: ProjectId | null
  /**
   * What cide knows about this project's subagents.
   *
   * Defaults to [`ROSTER_UNKNOWN`] — *nobody has looked* — and not to a disabled roster,
   * because a default that makes a claim is a claim made by omission. See the header.
   */
  roster?: Roster | undefined
  /**
   * Task id → title, for the agent→task link on a run row.
   *
   * A plain map rather than the board, so this panel does not have to know what a task is; a
   * missing key simply means the row draws its id without a title, which is what the model's
   * `titleOf` already decided.
   */
  taskTitles?: Readonly<Record<string, string>> | undefined
  /** The clock, as a prop. See the header. */
  nowMs: number
  /** Start a run of this role. Only offered when `canDispatch` says yes. */
  onDispatch?: ((agent: string) => void) | undefined
  onOpen?: ((run: string) => void) | undefined
  onPause?: ((run: string) => void) | undefined
  onResume?: ((run: string) => void) | undefined
  onStop?: ((run: string) => void) | undefined
  onRevealTask?: ((task: string) => void) | undefined
  onRetryTurn?: ((run: string) => void) | undefined
  onAckStaleTurn?: ((run: string) => void) | undefined
  /**
   * Write `enabled: true` into this project's `.cide/config.json`, creating it.
   *
   * Optional like every other handler: without one the button is **not rendered**, rather than
   * rendered dead. A control that does nothing when pressed is the failure this whole family
   * of panels keeps being rewritten to avoid.
   */
  onEnable?: (() => void) | undefined
  /** Show `.cide/` in the file manager, so the user can read what is about to be committed. */
  onRevealConfig?: (() => void) | undefined
  /**
   * **Open Settings on this role.** Drawn on every role row, unconditionally.
   *
   * The user's second sentence, and the counterpart of Open: a subagent that is doing nothing
   * offers no way to look at it, and offers this instead. It is on an `unavailable` role too —
   * a role whose harness is not installed is exactly the one somebody wants the settings for —
   * and on a role that has just been dispatched, because *configure* and *what is it doing* are
   * not alternatives.
   *
   * Optional like every other handler, and absent means the control is **not rendered** rather
   * than rendered dead. A host with no way to open Settings must not draw a button that claims
   * otherwise; `check-agents-render.mjs` asserts the control is there in every role row of every
   * story, which is only a real assertion because the fixture hands every story this handler.
   */
  onConfigure?: ((agent: string) => void) | undefined
  /**
   * Open Settings on the agents screen with no role named — the `empty` screen's one button.
   *
   * A second prop rather than `onConfigure(null)`, for `onPauseAll`'s reason one control over:
   * the two gestures differ in what they mean ("configure *this* subagent" against "I have none,
   * take me to where they are made"), and a nullable argument would let the screen-level gesture
   * be passed to a control drawn from a row.
   */
  onConfigureAll?: (() => void) | undefined
  /**
   * Freeze the **whole project**: every run, the dispatch queue, and this project's own
   * console session. `useAgents.pause()` with no argument.
   *
   * Not `onPause` with a run of `null`. The two gestures differ in what they reach — this one
   * reaches the pane the user is typing into — and a nullable argument would let a caller pass
   * the project scope to a control drawn from a row, which is the accident this pair of props
   * exists to make unwriteable. See [`PAUSE_ALL_TITLE`] for what the user is told.
   */
  onPauseAll?: (() => void) | undefined
  /**
   * Thaw everything and reopen the queue. `useAgents.resume()` with no argument.
   *
   * **The half that has to be reachable when nothing else is.** Its offer condition is
   * deliberately not the mirror of `onPauseAll`'s — see [`ScopeControl`], which is where the
   * asymmetry is argued and where a refactor will be tempted to tidy it away.
   */
  onResumeAll?: (() => void) | undefined
  /**
   * Which role's Integrate is armed — that is, has been clicked once and is waiting for the
   * confirming second click. `null` for none, which is the resting state.
   *
   * A **prop rather than state in this component**, and that is the file's oldest rule rather
   * than a preference: `AgentsPanel.tsx` is a function of its props so that
   * `check-agents-render.mjs` can render it under node and assert on the markup. A `useState`
   * here would make the armed screen — the one that matters, because it is the one directly in
   * front of a merge — the single state that gate could never see. `AgentsPanelHost` holds it,
   * which is also where it can be cleared when the project changes.
   *
   * Single-valued, so arming one role disarms any other by construction. Two roles armed at once
   * is not a state anybody wants and would be one more thing for a reader to hold.
   */
  integrateArmed?: string | null
  /**
   * First click: arm this role. Costs nothing, changes nothing, and is reversible by arming
   * another role or leaving the panel.
   *
   * The pair below is [`onPauseAll`]/[`onResumeAll`]'s shape — two handlers, at most one control
   * drawn, the condition in one place ([`IntegrateControl`]) — and for the same reason: the two
   * gestures differ in what they *do*, and one handler taking a boolean would let a caller pass
   * "yes, really merge" from the control that was only meant to ask.
   */
  onIntegrateArm?: ((agent: string) => void) | undefined
  /**
   * Second click: **merge `cide/<agent>` into the branch the user has checked out.**
   *
   * The one gesture this panel has that rewrites the user's working tree and adds a commit to
   * their history. It is only ever reachable from the armed control, so it cannot be reached by
   * a single press, and never for a role the user did not name — see [`IntegrateControl`].
   */
  onIntegrate?: ((agent: string) => void) | undefined
}

export function AgentsPanelView({
  project,
  roster = ROSTER_UNKNOWN,
  taskTitles = {},
  nowMs,
  onDispatch,
  onOpen,
  onPause,
  onResume,
  onStop,
  onRevealTask,
  onRetryTurn,
  onAckStaleTurn,
  onEnable,
  onRevealConfig,
  onConfigure,
  onConfigureAll,
  onPauseAll,
  onResumeAll,
  integrateArmed = null,
  onIntegrateArm,
  onIntegrate,
}: AgentsPanelViewProps) {
  /*
   * Not memoised, and that is a decision rather than an omission. `sections()` is a couple of
   * passes over a list bounded by the concurrency ceiling plus `RECENT_CAP`, and the panel
   * re-renders on the second because `nowMs` moves — so a `useMemo` keyed on the roster would
   * be recomputing on nearly every render anyway while adding a hook to a component whose
   * whole value is that it is a function of its props.
   */
  const body = sections(roster, taskTitles)
  const meta = metaFigure(roster)

  return (
    <aside className={styles.panel} data-audit="sidebarAgents" aria-label="Agents">
      <div className={styles.header} data-audit="agentsHeader">
        <span className={styles.headerTitle}>Agents</span>
        {/*
          * Empty, not `—`, when `metaFigure` returns `null`.
          *
          * The model argues this one out: an em dash means "we do not know", and this header
          * has two siblings on the rail that would each be printing one at the same moment —
          * three claims of ignorance where an empty slot is the honest render. What matters,
          * and what the check pins, is that it is never `0`: a zero here is a count, and
          * nobody counted.
          *
          * Withheld outright with no project open, for the same reason the body says so rather
          * than showing a roster: a figure counting runs in a project that is not on screen is
          * a number about nothing.
          */}
        <span className={styles.headerMeta} data-audit="agentsMeta">
          {project === null ? '' : (meta ?? '')}
        </span>
        {/*
          * The project scope, beside the figure it qualifies.
          *
          * Withheld with no project open on the same argument as the figure: a control that
          * would freeze "this project" when there is no project is a button with no object.
          */}
        {project === null ? null : (
          <ScopeControl roster={roster} onPauseAll={onPauseAll} onResumeAll={onResumeAll} />
        )}
      </div>

      {project === null ? (
        <div className={styles.body} data-audit="agentsBody">
          <p className={styles.claim}>No project open</p>
          <p className={styles.detail}>
            Subagents are configured per project. Open one from the header’s <b>+</b> button.
          </p>
        </div>
      ) : roster.kind === 'unknown' ? (
        /*
         * Nobody has looked. Draw nothing at all — see the file header. The body element still
         * exists so the panel keeps its width and the scroll container does not appear and
         * disappear under the first answer.
         */
        <div className={styles.body} data-audit="agentsBody" />
      ) : roster.kind === 'disabled' ? (
        <div className={styles.body} data-audit="agentsBody">
          <p className={cx(styles.claim, styles.claimOff)} data-audit="agentsClaim">
            {OFF_FOR_THIS_PROJECT}
          </p>
          <p className={styles.detail}>
            A subagent is a second Claude (or opencode) run under a role you define —
            developer, qa, artist — dispatched by the session in your console tab. They run
            headless; nothing opens a pane unless you ask it to.
          </p>
          {/*
            * The consequence, before the button and not after it.
            *
            * Enabling writes a file into the user's repository, and a toggle that quietly adds
            * a tracked file is a surprise commit. The path is printed whole and never elided:
            * it is the one word in the sentence that cannot be guessed.
            */}
          <p className={styles.warn} data-audit="agentsWarn">
            Turning them on writes this file, which your repository will contain:
            <code className={styles.path} data-audit="agentsPath">
              {roster.configPath}
            </code>
          </p>
          <p className={styles.detail}>
            Roles are defined beside it, in <code>.cide/agents/*.md</code>. cide never invents
            one.
          </p>
          {/* The backend's own sentence about *why* it is off — a project that is not a git
              repository cannot use worktree isolation, and that is worth saying here rather
              than only on the refusal. */}
          {roster.hint.trim() !== '' && (
            <p className={styles.detail} data-audit="agentsHint">
              {roster.hint}
            </p>
          )}
          <div className={styles.actions} data-audit="agentsActions">
            {onEnable !== undefined && (
              <button
                type="button"
                className={cx(styles.action, styles.actionPrimary)}
                data-audit="agentsEnable"
                onClick={onEnable}
              >
                Enable subagents for this project
              </button>
            )}
            {onRevealConfig !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="agentsReveal"
                onClick={onRevealConfig}
              >
                Reveal .cide/
              </button>
            )}
          </div>
        </div>
      ) : roster.kind === 'empty' ? (
        /*
         * Subagents are on and there are no roles. **One centred button, into Settings.**
         *
         * What used to be here was a worked example of a role file — front matter, a system
         * prompt, the lot — which is the panel answering "you have no subagents" with "here is
         * some YAML to write". The user who ran it said so. The screen for having none is the
         * one that takes you to where they are made, and everything else on it is in the way of
         * that, so everything else is gone: no `warn` box, no path, no example. Centred rather
         * than top-aligned because it is the only thing on the screen, and a lone button at the
         * top of an empty column reads as the first item of a list that failed to load.
         *
         * The claim above it stays. A button with no sentence is a screen that has not said what
         * state the user is in, and "on, but nothing defined" is genuinely different from "off"
         * — which is the whole reason `AgentRoster` has two arms rather than one.
         */
        <div className={styles.body} data-audit="agentsBody">
          <div className={styles.centre} data-audit="agentsEmpty">
            <p className={styles.claim} data-audit="agentsClaim">
              No subagents yet.
            </p>
            <p className={styles.detail}>
              Subagents are on for this project. Define one and it can be dispatched from here.
            </p>
            {onConfigureAll !== undefined && (
              <button
                type="button"
                className={cx(styles.action, styles.actionPrimary)}
                data-audit="agentsConfigureAll"
                onClick={onConfigureAll}
              >
                {CONFIGURE_ALL_LABEL}
              </button>
            )}
            {/*
              * Second, and quiet. A role is a committed file, so the route to the directory is
              * worth keeping — but it is not the answer to "I have no subagents", and a second
              * control of equal weight beside the primary one is two things to choose between
              * on a screen whose whole job is to offer one.
              */}
            {onRevealConfig !== undefined && (
              <button
                type="button"
                className={styles.link}
                data-audit="agentsReveal"
                onClick={onRevealConfig}
              >
                Reveal .cide/
              </button>
            )}
          </div>
        </div>
      ) : (
        <div className={styles.body} data-audit="agentsBody">
          <div className={styles.sections} data-audit="agentsSections">
            {body.map((section) =>
              section.kind === 'recent' ? (
                /*
                 * Recent is a real `<details>`: the collapse costs no state, which is what lets
                 * this component stay a function of its props, and it survives every re-render
                 * the moving clock causes. Closed by default — the panel answers "what is
                 * happening", and history is the thing you go and look for.
                 *
                 * Its rows are `RunRow`s and not role rows, and that is the section's whole
                 * point: a finished run has no bearing on what its role is doing now, so it is
                 * off the role row entirely — and this is the only place its transcript is
                 * still reachable from. `PtySession` feeds its vt100 mirror whether or not
                 * anything is attached, so Open here really does bring back the screen.
                 */
                <details className={styles.recent} data-audit="agentsSection" key={section.kind}>
                  <summary className={styles.recentSummary}>
                    <span className={styles.sectionTitle}>{section.label}</span>
                    <span className={styles.sectionCount}>{countLabel(section)}</span>
                  </summary>
                  <div className={styles.rows} data-audit="agentsRows">
                    {section.rows.map((row) => (
                      <RunRow
                        key={row.run.run}
                        row={row}
                        nowMs={nowMs}
                        onOpen={onOpen}
                        onStop={onStop}
                        onRevealTask={onRevealTask}
                      />
                    ))}
                  </div>
                </details>
              ) : (
                <div className={styles.section} data-audit="agentsSection" key={section.kind}>
                  <div className={styles.sectionHead}>
                    <span className={styles.sectionTitle}>{section.label}</span>
                    <span className={styles.sectionCount}>{countLabel(section)}</span>
                  </div>
                  <div className={styles.rows} data-audit="agentsRows">
                    {section.rows.map((role) => (
                      <RoleLine
                        key={role.def.id}
                        role={role}
                        nowMs={nowMs}
                        onDispatch={onDispatch}
                        onConfigure={onConfigure}
                        onOpen={onOpen}
                        onPause={onPause}
                        onResume={onResume}
                        onStop={onStop}
                        onRevealTask={onRevealTask}
                        onRetryTurn={onRetryTurn}
                        onAckStaleTurn={onAckStaleTurn}
                        armed={integrateArmed === role.def.id}
                        onIntegrateArm={onIntegrateArm}
                        onIntegrate={onIntegrate}
                      />
                    ))}
                  </div>
                </div>
              ),
            )}
          </div>
        </div>
      )}
    </aside>
  )
}

/**
 * The figure beside a section heading — how many subagents, or how many finished runs.
 *
 * Recent additionally says when it is showing a prefix rather than the whole history: a
 * counter that reports its own cap as the total is the same quiet lie as an unchecked zero,
 * which is the failure `ProblemsPanel` was written against.
 */
function countLabel(section: Section): string {
  if (section.kind === 'recent' && section.rows.length >= RECENT_CAP) {
    return `last ${section.rows.length}`
  }
  return String(section.rows.length)
}

/**
 * Is anything in this project frozen right now — **by any route, including the one that leaves
 * no row behind?**
 *
 * Two facts, and the second is the one a reader deletes by mistake:
 *
 *  * a run reading `paused` — the ordinary case, and the only one visible in the list;
 *  * `dispatching === false` — the project's queue is shut, which only a project-scope pause
 *    does. `AgentRegistry::dispatching` reads exactly one thing, `paused_projects`, and a
 *    per-run pause never touches it, so this flag *is* "a project-scope pause is in effect".
 *
 * The wire carries `dispatching` beside the run states for this and for nothing else: a project
 * whose queue is shut with every run finished is byte-identical to an idle one in the `runs`
 * array, and in that state the user's **console pane is frozen and every row on screen says
 * `finished`**. Deriving frozen-ness from the rows alone loses exactly that case, which is the
 * case where the user has no other way back — the pane they would type a command into is the
 * pane the pause stopped.
 *
 * A roster that is not `ready` answers `false` because it carries no fact to read, not because
 * a non-ready project cannot be paused: `disabled`, `empty` and `unknown` have neither `runs`
 * nor `dispatching` on them. That is safe in practice — `agents.roster` is served by the Rust
 * registry and not by the frozen console, so a paused project keeps answering `Ready` — and if
 * a later arm ever carries a project-level pause flag, it belongs here.
 */
function anythingFrozen(roster: Roster): boolean {
  if (roster.kind !== 'ready') return false
  return !roster.dispatching || roster.runs.some((run) => run.phase === 'paused')
}

/**
 * Is there anything for a project-scope pause to act on?
 *
 * **A run that has not ended, with the queue still open.** Both halves are the gesture's two
 * halves: `AgentRegistry::pause` shuts the queue *and* freezes the children, so it has work to
 * do while either a child exists or the queue could still start one.
 *
 * `!isDonePhase` and not `canPause`, which is the row control's gate and a narrower question.
 * `canPause` excludes `idle` because freezing an idle *child* buys nothing; the project scope
 * is not about that child at all — it is about the console that is about to write the next turn
 * into it, and shutting the queue that is about to start another run beside it. `queued` is in
 * for the same reason and is the clearest case: there is no process to `SIGSTOP`, and holding
 * it in the queue is the entire point.
 *
 * The condition deliberately does **not** reduce to "a project is open". With no run at all and
 * the queue open, the only thing a pause would do is freeze the user's own console — the
 * documented side effect standing alone as the whole effect, on a screen with no agents on it.
 * That is a control the Agents panel has no business drawing; the `agents.pause` palette row is
 * still there for a user who genuinely wants it.
 */
function anythingToFreeze(roster: Roster): boolean {
  if (roster.kind !== 'ready') return false
  return roster.dispatching && roster.runs.some((run) => !isDonePhase(run.phase))
}

/**
 * The header's project-scope control: **at most one button, and which one is the design.**
 *
 * # Resume is not the mirror of Pause, and the asymmetry is load-bearing
 *
 * A project-scope pause freezes every run *and the project's own console session* — the pane
 * the user types into. So the resume half has to be reachable from a window where nothing else
 * is: it is offered whenever [`anythingFrozen`] says something is frozen, and on **no** other
 * condition. Not gated on the roster being busy, not on there being a live run, not on
 * `dispatching`, not on a row being visible. Every one of those can be false while something is
 * frozen, and a Resume that is absent while the console is frozen is a window with no way out
 * of a state the app itself put it in. `crates/cide-app/src/agents.rs` states the same rule from
 * the other end — "Resume must be reachable from a control that is not the frozen pane" — and
 * `AgentRoster::Ready::dispatching` exists on the wire only so that this function can be right.
 *
 * # Never both, never as equals
 *
 * Both can be meaningful at once: one run paused by its own row control while another runs and
 * the queue is open. **Resume wins the slot**, unconditionally, because the cost of the two
 * mistakes is not symmetric — a missing Pause is a gesture the user makes from the palette or
 * from the row, and a missing Resume can be a window that has stopped answering the keyboard.
 * Two buttons side by side in a 30px header would also be two 20px controls in the space the
 * meta figure shares, which is how the header stops reading at a glance.
 *
 * Either handler being absent withholds its half outright rather than drawing it dead, which is
 * the rule every other control in this panel follows.
 */
function ScopeControl({
  roster,
  onPauseAll,
  onResumeAll,
}: {
  roster: Roster
  onPauseAll?: (() => void) | undefined
  onResumeAll?: (() => void) | undefined
}) {
  if (onResumeAll !== undefined && anythingFrozen(roster)) {
    return (
      <button
        type="button"
        className={cx(styles.headerAction, styles.headerActionResume)}
        data-audit="agentsResumeAll"
        /* `title` and `aria-label` carry the same sentence — `Explorer.tsx`'s treatment, for
           the same reason: a control whose whole job is to be found must say what it does in
           the one place a hover or a screen reader will look. The visible word is inside the
           sentence, so the accessible name still contains the label. */
        title={RESUME_ALL_TITLE}
        aria-label={RESUME_ALL_TITLE}
        onClick={onResumeAll}
      >
        {/* Hidden from the accessible name so it reads "Resume …" rather than "▶ Resume …".
            The word beside it is not decoration — see the stylesheet. */}
        <span className={styles.headerActionGlyph} aria-hidden="true">
          <Icon name="play" size={1} />
        </span>
        Resume
      </button>
    )
  }
  if (onPauseAll !== undefined && anythingToFreeze(roster)) {
    return (
      <button
        type="button"
        className={styles.headerAction}
        data-audit="agentsPauseAll"
        title={PAUSE_ALL_TITLE}
        aria-label={PAUSE_ALL_TITLE}
        onClick={onPauseAll}
      >
        <span className={styles.headerActionGlyph} aria-hidden="true">
          <Icon name="pause" size={1} />
        </span>
        Pause
      </button>
    )
  }
  return null
}

/**
 * **One subagent.** The name, what it is doing right now, one line per active run, and the
 * role-scope controls — of which Configure is the only one that is always there.
 *
 * ```
 * ● Developer · Running                    1/1
 *     ● t-14 Add the retry bar    2m   Open ⏸ ⏹
 *   Implements one task end to end and reports back on it.
 *   Dispatch   Integrate   Configure
 * ```
 *
 * # The two controls the user asked for
 *
 * **Open is not drawn here at all.** It belongs to a *run* — it mirrors that run's session into
 * a pane — so it is drawn by [`ActivityRow`], once per active run, from the same `canOpen` the
 * model put on the row. A role with nothing active has no activity lines, so it has no Open
 * element anywhere: withheld, not disabled, which is this panel's rule and is here made
 * structural rather than conditional. `role.canOpen` is the same fact one level up, for anything
 * that needs to ask.
 *
 * **Configure is always drawn**, whatever the role is doing and whether or not it can be
 * dispatched. It is the one control on the row that leads to where the role can be *changed*,
 * and the row that most needs it is the one whose role is broken.
 *
 * # Never both, never neither
 *
 * The Dispatch button and the refusal sentence are derived from `role.dispatch`, which
 * `sections()` produced with a single `canDispatch` call — and that call's return type has no
 * third shape: a green light, or a non-empty sentence. Building the two nodes from the one value
 * in two adjacent statements is what makes "a greyed control with nothing explaining it"
 * unwriteable here rather than merely discouraged; `check-agents.mjs` holds the model to the
 * same invariant from the other side.
 *
 * The one arrangement that draws neither is a host that passed no `onDispatch` at all. That is
 * not a state the app has — the panel exists to dispatch — and the alternative, drawing the
 * button anyway, is a control that does nothing when pressed, which is the worse half of the
 * trade and the failure this family of panels keeps being rewritten to avoid.
 */
function RoleLine({
  role,
  nowMs,
  onDispatch,
  onConfigure,
  onOpen,
  onPause,
  onResume,
  onStop,
  onRevealTask,
  onRetryTurn,
  onAckStaleTurn,
  armed,
  onIntegrateArm,
  onIntegrate,
}: {
  role: RoleRow
  nowMs: number
  onDispatch?: ((agent: string) => void) | undefined
  onConfigure?: ((agent: string) => void) | undefined
  onOpen?: ((run: string) => void) | undefined
  onPause?: ((run: string) => void) | undefined
  onResume?: ((run: string) => void) | undefined
  onStop?: ((run: string) => void) | undefined
  onRevealTask?: ((task: string) => void) | undefined
  onRetryTurn?: ((run: string) => void) | undefined
  onAckStaleTurn?: ((run: string) => void) | undefined
  /** Is *this* role the one waiting for a confirming second click? See `integrateArmed`. */
  armed: boolean
  onIntegrateArm?: ((agent: string) => void) | undefined
  onIntegrate?: ((agent: string) => void) | undefined
}) {
  const { def, dispatch } = role
  const button =
    dispatch.ok && onDispatch !== undefined ? (
      <button
        type="button"
        className={styles.action}
        data-audit="agentsDispatch"
        onClick={() => onDispatch(def.id)}
        title={`Start a ${def.label} run.`}
      >
        Dispatch
      </button>
    ) : null
  const reason = dispatch.ok ? null : (
    <p className={styles.roleReason} data-audit="agentsRoleReason">
      {dispatch.reason}
    </p>
  )

  return (
    <div
      className={styles.roleRow}
      data-audit="agentsRole"
      data-role={def.id}
      /* The summary phase, or `none`. On the element rather than only in the words, because the
         render check reads it: "this role is doing nothing" is the state the Open rule turns on,
         and asserting it through a rendered English word would be asserting the wording. */
      data-phase={role.runs[0]?.run.phase ?? 'none'}
    >
      <div className={styles.roleTop}>
        {/*
          * The summary dot: the glyph and tone of the role's most demanding active run, or the
          * resting mark when there is none. `model.ts` decided both — a second table here would
          * be a second set of colours for one vocabulary, and `TONE_CLASS` is imported from
          * `RunRow.tsx` for the same reason.
          */}
        <span
          className={cx(styles.glyph, TONE_CLASS[role.tone])}
          data-audit="agentsRoleGlyph"
          data-tone={role.tone}
          aria-hidden="true"
        >
          <Icon name={asIcon(role.glyph)} size={1} />
        </span>
        <span className={styles.roleName} title={def.id}>
          {def.label}
        </span>
        {/*
          * Where the definition came from, but **only when that is a surprise** — see
          * `scopeBadge`, which returns a mark for the two Claude Code scopes and `null` for
          * cide's own two. Immediately after the name because it qualifies the name: this row
          * names a subagent `claude` also knows about outside cide, with no harness to choose
          * and a definition cide applies through `--agent` rather than through its own flags.
          *
          * `title` carries the longer sentence rather than the row: the roster is a list to scan,
          * and a paragraph on every subagent row would push the runs off the bottom.
          */}
        {scopeBadge(def.scope) !== null && (
          <span
            className={styles.roleBadge}
            data-audit="agentsRoleBadge"
            data-scope={def.scope}
            title={
              def.scope === 'claudeGlobal'
                ? 'A Claude Code subagent from ~/.claude/agents — cide runs it with `claude --agent`.'
                : 'A Claude Code subagent from this project’s .claude/agents — cide runs it with `claude --agent`.'
            }
          >
            {scopeBadge(def.scope)}
          </span>
        )}
        <span className={styles.sep} aria-hidden="true">
          ·
        </span>
        {/*
          * What this subagent is doing, **in words**. The dot beside it says the same thing in a
          * glyph, and the words are what makes the row answer the user's question without
          * having learned the glyphs — which was the whole complaint about the five-section
          * layout this replaced.
          */}
        <span className={styles.roleStatus} data-audit="agentsRoleStatus">
          {role.status}
        </span>
        {/* `live/max`, plus the queue depth when there is one. A role sitting at its ceiling
            and a role with three runs waiting behind it are different situations, and a single
            figure would show them as the same one. */}
        <span className={styles.roleFigure} data-audit="agentsRoleFigure">
          {role.live}/{def.maxConcurrent}
          {role.queued > 0 ? ` +${role.queued}` : ''}
        </span>
      </div>

      {/*
        * One line per active run. Empty for a resting role, which is exactly why that role has
        * no Open control: there is no line for one to sit on.
        *
        * Every run is listed rather than only the first — `RoleRow.runs` argues it out — because
        * a run that is on screen nowhere is a `claude` spending the user's quota with no way to
        * see, open or stop it. Under the default worktree isolation there is at most one.
        */}
      {role.runs.map((row) => (
        <ActivityRow
          key={row.run.run}
          row={row}
          nowMs={nowMs}
          onOpen={onOpen}
          onPause={onPause}
          onResume={onResume}
          onStop={onStop}
          onRevealTask={onRevealTask}
          onRetryTurn={onRetryTurn}
          onAckStaleTurn={onAckStaleTurn}
        />
      ))}

      {def.description.trim() !== '' && (
        <p className={styles.roleDesc} data-audit="agentsRoleDesc">
          {def.description}
        </p>
      )}
      {reason}

      <div className={styles.roleActions} data-audit="agentsRoleActions">
        {button}
        {/*
          * Beside Dispatch, and independent of it: *start work* and *take the work back* are the
          * two ends of a role's life and a row that offered only the first would be half a loop.
          * The independence is the point — see [`IntegrateControl`], which is offered on a role
          * that cannot be dispatched at all.
          */}
        <IntegrateControl
          agent={def.id}
          label={def.label}
          armed={armed}
          onIntegrateArm={onIntegrateArm}
          onIntegrate={onIntegrate}
        />
        {/*
          * Last, and on every row. It is last because it is the control you reach for when the
          * other two are not what you want, and it is on every row because *what this role is*
          * is always editable — including, and especially, when the role is unavailable and
          * neither of the others can do anything at all.
          */}
        {onConfigure !== undefined && (
          <button
            type="button"
            className={styles.action}
            data-audit="agentsConfigure"
            onClick={() => onConfigure(def.id)}
            title={CONFIGURE_TITLE}
            aria-label={`Configure ${def.label} in Settings`}
          >
            Configure
          </button>
        )}
      </div>
    </div>
  )
}

/**
 * A role row's **Integrate**: at most one button, and which one is the whole design.
 *
 * # When it is offered: always, and that is an honest answer rather than a lazy one
 *
 * A role with no worktree, or whose branch holds nothing the base lacks, has nothing to
 * integrate — and **the wire does not carry that fact**. `AgentDef` describes a file in
 * `.cide/agents/`: an id, a label, a harness, a description, a prompt, a ceiling and a fault. It
 * says nothing about `cide/<role>`, whether that branch exists, or how far ahead of `HEAD` it
 * is. Nothing in `AgentRoster` does either.
 *
 * So there were two options. Derive a guess from the roster — "this role has a finished run, so
 * it probably has commits" — or offer it always and let the backend answer. The guess is wrong
 * in both directions: a run that finished having written nothing leaves an empty branch, and a
 * role whose runs are long gone from `RECENT_CAP` still has every commit it ever made. A control
 * offered on a guess is a control that is sometimes missing over work that is really there,
 * which is the worse of the two failures by a distance.
 *
 * So: **offered on every role, unconditionally**, and `UpToDate` is a perfectly good answer that
 * the host reports as one. This is not a predictive button and must not be described as one; it
 * is a question the user is allowed to ask, whose answer sometimes is "nothing to do".
 *
 * **What it merges narrowed when worktrees went per-task**: this button names no task, so it
 * targets the role's *base* branch `cide/<role>` — the one its taskless dispatches commit to.
 * A task's work lives on `cide/<role>-<task>` now, and the road to it is the orchestrator's
 * `cide_agent_integrate` with the task named; a merge control on the task's own card is the
 * named follow-up for doing it by hand. The backend's "no branch yet" refusal spells the exact
 * branch it looked for, so pressing this over per-task work reads as an explanation rather
 * than a silent no-op.
 *
 * Two consequences worth stating because each looks like an oversight:
 *
 *  * **A role that cannot be dispatched still gets it.** `unavailable` is a fact about the
 *    machine — the harness binary is not on `PATH`, the definition file has no body — and none
 *    of those makes the commits the role already pushed to its branch less real. Withholding the
 *    control there would strand finished work behind an uninstalled CLI.
 *  * **A frozen project still gets it.** A project-scope pause stops children; this merges a
 *    branch in the user's own checkout, which is exactly the thing somebody does *while* the
 *    agents are held.
 *
 * # Two clicks, and why this one earns them
 *
 * The first press arms; the second merges. `ConfirmDestructive` — the house dialog — is for acts
 * that are irreversible or that were triggered by something outside the user's intent, and this
 * is neither: a merge is recoverable (`ORIG_HEAD`, the reflog) and it is only ever reached by
 * pressing a button labelled *Integrate*. But it writes files the user may have open and adds a
 * commit to their branch, from a row in a scrolling list, one row's height from *Dispatch*. That
 * is "recoverable but real, and easy to hit by mistake", which is the confirm-on-second-click
 * case rather than the dialog case.
 *
 * **The arm is per role and the merge is only ever for the role that was armed.** `agent` is
 * closed over from the row that drew the control, so "never merge without the user having asked
 * for this specific role" is structural here rather than a rule somebody has to keep.
 *
 * # Never both, never dead
 *
 * Exactly one of the two buttons exists at a time, and a missing handler withholds its half
 * outright rather than drawing it disabled — the rule every other control in this panel follows,
 * and the reason `check-agents-render.mjs` counts elements rather than `disabled` attributes.
 */
function IntegrateControl({
  agent,
  label,
  armed,
  onIntegrateArm,
  onIntegrate,
}: {
  agent: string
  /** The role's display name, for the confirming button's accessible sentence. */
  label: string
  armed: boolean
  onIntegrateArm?: ((agent: string) => void) | undefined
  onIntegrate?: ((agent: string) => void) | undefined
}) {
  if (armed && onIntegrate !== undefined) {
    return (
      <button
        type="button"
        className={cx(styles.action, styles.actionArmed)}
        data-audit="agentsIntegrateConfirm"
        /* The role is named in the accessible sentence and not only in the row above it: this is
           the press that merges, and a screen reader on a list of five roles must be able to say
           *which* branch is about to land without moving off the control. `ScopeControl`'s
           treatment, same reason. */
        title={INTEGRATE_CONFIRM_TITLE}
        aria-label={`Merge ${label}’s branch now. ${INTEGRATE_CONFIRM_TITLE}`}
        onClick={() => onIntegrate(agent)}
      >
        Confirm merge
      </button>
    )
  }
  if (onIntegrateArm !== undefined) {
    return (
      <button
        type="button"
        className={styles.action}
        data-audit="agentsIntegrate"
        title={INTEGRATE_TITLE}
        aria-label={`Integrate ${label}’s work. ${INTEGRATE_TITLE}`}
        onClick={() => onIntegrateArm(agent)}
      >
        Integrate
      </button>
    )
  }
  /*
   * Armed with no `onIntegrate`, or neither handler at all. Nothing is drawn — not a disabled
   * *Confirm merge*, which would be a control that has already asked the user for a decision and
   * then cannot act on it. Not a state the app has: `AgentsPanelHost` passes the pair together.
   */
  return null
}
