/**
 * This project's subagent roster, and what keeps it fresh. (M18)
 *
 * Modelled on `tasksStore` and `diagnosticsStore`, because it answers the same shape of
 * question: a per-project snapshot that several surfaces read — the Agents panel, and the
 * activity rail's ⌬ badge — refreshed on an event rather than on a timer.
 *
 * # `adopt` takes the payload, and there is no `rev` to check
 *
 * `cide://agents-changed` carries the **whole roster**, so the handler adopts it and never asks
 * Rust anything: answering an event that just told you the answer with a round trip asking for
 * it is a question with a known answer. `cide://diagnostics` is a *hint* — it says something
 * moved and costs a call to act on — which is why `diagnosticsStore` coalesces and this does
 * not.
 *
 * And unlike `cide://tasks-changed` there is **no `rev` on the envelope, and none is needed**.
 * `.cide/tasks.json` genuinely has several writers — this window, another window, and every
 * dispatched agent through the orchestration MCP server — so two board snapshots can arrive out
 * of order and `newerBoard` exists to drop the older one. The roster has exactly one writer: the
 * in-process registry, behind its own lock, which emits after it has taken the state. Emits
 * therefore leave in the order the state took, and the last one to arrive is the newest by
 * construction. A counter here would be a comparison with nothing to compare.
 *
 * That leaves one ordering hazard, and it is the *other* direction: a slow `agents_roster`
 * answer landing after a broadcast that already carried newer state. `refresh` is
 * generation-guarded for exactly that, and the guard is claimed before the call rather than
 * after — see below.
 *
 * # The subscription is in `App.tsx`, not in the panel
 *
 * `gitCountStore`'s stated reason: the rail's ⌬ badge has to stay live while the sidebar is shut
 * or showing Files, and a listener registered inside a panel goes stale the moment that panel
 * unmounts. A user who closes the sidebar would otherwise see the count freeze at whatever it
 * was when they closed it — a number that looks current and is not.
 *
 * # The run gestures, and why each of them ends in a `refresh`
 *
 * `dispatch`, `stop`, `pause`, `resume`, `retryTurn`, `ackStaleTurn` and `openPane` do their
 * work and then re-ask. That looks redundant beside `adopt` — the registry broadcasts
 * `cide://agents-changed` after every one of them — and it is not, for two separate reasons.
 *
 * The first is timing. That broadcast is **coalesced at ~120 ms in Rust**, which is right for a
 * stream of phase changes and wrong for the frame after a click: a Dispatch button whose row
 * does not move for an eighth of a second reads as a button that did nothing, and the second
 * press is a second run. So the refresh is what moves the screen *at* the gesture. Nothing here
 * adds a debounce of its own — a second one on this side would stack with Rust's and put the
 * answer further away rather than nearer.
 *
 * The second is that **none of them optimistically touches `roster`**. Only the registry
 * knows whether a dispatch started or queued, and a store that guessed would draw the role
 * reading `Running` for a frame and then correct it to `Queued` — which is not a cosmetic
 * flicker but a claim that a `claude` was spawned when none was. `stop` is the same shape from the
 * other end: a run removed here would vanish before its child had died, and the transcript in
 * Recent is the record of what the agent did. `pause` is the sharpest case of all: the registry
 * decides which sessions were actually frozen — a run that finished between the paint and the
 * click is not one of them — and a row drawn `paused` on this side would be a claim that a
 * child is stopped when the kernel was never asked.
 *
 * # `openPane` is the one that is not a command, and that is the whole of the M18 mirror bug
 *
 * All the others send a `#[tauri::command]` and wait. `openPane` does not: it hands the run to
 * `AgentsPanel/openRun.ts`, which adds a row to the project console with `SplitIntent::Mirror`
 * and lets the ordinary split machinery finish it. A pane built in the domain instead — which is
 * what a short-lived `agent_open_pane` command did, since deleted — arrives over
 * `cide://workspace-changed` with no spawn plan, so
 * `TerminalPane`'s mirror branch never runs, `PaneHost.mirrored` is never set, and `closePane`
 * cannot tell the pane from one that owns its child: **closing it would kill the agent
 * mid-turn.** `openRun.ts`'s header has the argument in full, and it is the reason this gesture
 * is allowed to be the odd one out.
 *
 * # Pause takes a nullable run, and the project scope freezes the user's own console
 *
 * `pause()` and `resume()` with no argument are the **project scope**: the dispatch queue is
 * shut and every live run is frozen, *including the project's primary console session*. That is
 * `agents_pause`'s design and not an accident — a pause that left the orchestrator running
 * would leave it dispatching into frozen children — and it has one consequence this store
 * cannot fix on its own: **resume must be reachable from a control that is not the frozen
 * pane.** The Agents panel and the `agents.resume` palette row are those controls. A store that
 * offered pause-all with no resume outside the pane would be handing the user a switch that
 * turns their console off permanently.
 *
 * # `configure` leaves the panel, and the role it names has nowhere to be delivered yet
 *
 * The Agents panel's Configure control opens the project's **Settings** tab on the agents
 * screen: `settings.openTab(project, 'agents')`, which is an existing command with an existing
 * `SettingsSection::Agents` variant. That much works today and is the whole of what the control
 * promises.
 *
 * What the argument itself cannot do is open that screen *on a particular role*, because it
 * selects a **screen** and there is no field on it for a row. `focusRole` below is this store's
 * half of that: the id is recorded before the tab is opened, and `settings/AgentsSection.tsx`
 * takes it through [`takeFocusRole`] — subscribing to the field, taking it once, and holding it
 * until its own listing has arrived. The **scope** is the one thing this store cannot supply —
 * `AgentDef` on the wire does not say whether a definition is the project's or the user's, which
 * that file's own header says at length — so the id is what crosses, and finding which scope
 * holds it is work that belongs on the side that lists both. `agentsDraft.ts`'s `focusTarget`
 * does it, and prefers the *project* file when both scopes hold the name, because that is the
 * one a dispatch of that name actually runs.
 *
 * **The gesture stays honest if that screen ever stops reading it**: the user lands on the
 * screen that lists their subagents and picks one, which is one click more than the ideal and
 * zero clicks worse than a control that did not exist.
 *
 * It is deliberately a field of *this* store rather than something new in `settings/`: a store
 * this slice owns can be read by that screen whenever it wants to, and editing a file another
 * change was actively writing would have been two owners for one file.
 *
 * # `retryTurn` and `ackStaleTurn` are two gestures, not one with a flag
 *
 * A resumed run whose turn may have died under the freeze carries `staleTurn`, and the two
 * answers to it are *retry* — which **spends a turn of the user's quota** — and *leave it*,
 * which spends nothing. `agentRuns.retryTurn` states why they stay two calls all the way down:
 * a boolean between the user and a call that costs them money is the wrong shape.
 */
import { create } from 'zustand'
import {
  agentRuns as agentRunsApi,
  agents as agentsApi,
  agentWorktree as agentWorktreeApi,
  settings as settingsApi,
  type AgentId,
  type AgentIntegration,
  type AgentRoster as WireRoster,
  type DispatchRequest,
  type OrchestrationConfig,
  type ProjectId,
  type RunId,
  type TaskId,
} from '@/ipc/client'
import { adaptRoster } from './AgentsPanel/adapt'
import { ROSTER_UNKNOWN, type Roster } from './AgentsPanel/model'

interface AgentsStore {
  project: ProjectId | null
  /** What cide last knew about this project's subagents. **Never null** — see `ROSTER_UNKNOWN`. */
  roster: Roster
  /**
   * `.cide/config.json` as it stands, or `null` when nobody has looked.
   *
   * Held beside the roster rather than derived from it because the two answer different
   * questions: the roster says what a project *has*, the config says what its ceilings and
   * default harness are. `null` here is the same "nobody looked" the roster's `unknown` arm is,
   * and it is deliberately not defaulted to `{ enabled: false, … }` — a default that makes a
   * claim is a claim made by omission, and this one would say a project's subagents are off.
   */
  config: OrchestrationConfig | null
  /**
   * Which role the Agents **settings screen** should open on, or `null` for none.
   *
   * Set by [`configure`] immediately before the Settings tab is opened, and cleared on `attach`
   * so a role id cannot outlive the project it belongs to. Read through [`takeFocusRole`] and
   * never directly: the value is a *request*, and a request that survives being read is a bug.
   *
   * `null` is *"open the screen, no row in particular"* — the empty panel's centred button — and
   * not "we could not tell". There is no third state to distinguish, because the only writer is
   * a click that either named a role or did not.
   */
  focusRole: AgentId | null

  /**
   * Take the pending focus request, clearing it. `null` when there is none.
   *
   * # Read once, exactly, and the precedent is `layout/spawnPlans.ts`
   *
   * `takeSpawnPlan` removes a plan from its map on read, and its header says why in one line:
   * React 19's StrictMode mounts effects twice in development, so a value left in place is
   * applied twice — there, two `claude` children sharing a parent. The failure here wears a
   * different costume and is the same shape. A `focusRole` left set after the settings screen
   * has honoured it is applied again the **next** time that screen mounts — a section switch, a
   * tab switch, a relaunch that restores the Settings tab — and it jumps the form to a role the
   * user has not asked about since, throwing away whatever they had open to do it. A focus
   * request is a fact about one click, exactly like a spawn plan is a fact about one split.
   *
   * `set` is skipped when there is nothing to clear, so the ordinary "no request pending" read —
   * which the screen performs on every mount — does not notify a single subscriber.
   *
   * The **scope** is deliberately not part of the answer, and cannot be: `AgentDef` on the wire
   * does not say whether a definition is the project's or the user's, because the roster is
   * merged. Finding which file the name belongs to is work for the side that lists both, and
   * `settings/agentsDraft.ts`'s `focusTarget` is where it happens.
   */
  takeFocusRole: () => AgentId | null

  /** Point the store at a project, or at nothing. Clears first. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Ask Rust for the roster and the config again. The tail of `attach`, and of `enable`. */
  refresh: () => Promise<void>
  /** Take a roster somebody else produced — a `cide://agents-changed` broadcast. */
  adopt: (project: ProjectId, roster: WireRoster) => void
  /**
   * Take a config somebody else wrote — Settings → Agents' project switches.
   *
   * # Why that screen writes through `agentsApi` and hands the answer back here
   *
   * Every gesture in this store acts on `get().project`, the project the sidebar is attached to.
   * The settings screen takes its project from `useActiveProject` instead, and in a second window
   * those two can name different projects — `focusRole`'s guard one field up exists for exactly
   * that. A `setMaxConcurrent()` on this store would therefore have written a *tracked file into
   * a repository the user was not looking at*, which is the one class of mistake this feature
   * cannot make. So the write is the screen's, with its own project spelled out, and only its
   * result comes here.
   *
   * Guarded on the project for `adopt`'s reason, and it is the same guard: an answer about a
   * project this store is not showing is a number about a different repository. `cide://agents-
   * changed` cannot replace this — it carries the roster and no config, deliberately, because it
   * fires several times a second as runs move.
   */
  adoptConfig: (project: ProjectId, config: OrchestrationConfig) => void
  /** Write `enabled: true` into `.cide/config.json`, creating it. The panel's one button. */
  enable: () => Promise<void>
  /**
   * **Open Settings on the agents screen**, on `agent` when one is named.
   *
   * The one gesture in this store that does not touch a run, a role file or the registry: it
   * opens a tab. It is here rather than in the panel because the panel's host funnels every
   * gesture through one error path, and because `focusRole` — the half a later Settings slice
   * will read — has to be set by the same call that opens the tab, or the two can disagree by a
   * frame.
   *
   * Rejects when `tab_open_settings` does, like every other gesture here; `AgentsPanelHost` puts
   * the sentence on screen through `notifyFailure`. A Configure that silently did nothing would
   * be the dead control this whole panel is built to make unrepresentable — and it is the only
   * control on a role row that is offered unconditionally, so it is the one that must work in
   * every state.
   */
  configure: (agent?: AgentId) => Promise<void>

  /**
   * Start a run of `agent`, against `task`, with `prompt` as this run's extra instruction.
   *
   * `task` is optional on the wire and the panel is expected to fill it anyway — a run with no
   * task is a run the Tasks panel cannot account for. The signature keeps it optional rather
   * than required because an ad-hoc run is a legal thing for the orchestrator to ask for, and a
   * store that forbade what the wire allows would be a second, quieter rule.
   *
   * Resolves once the run is **on the queue**, not once it has started; `agentRuns.dispatch`
   * says why at length. The run id is deliberately dropped: nothing in this window can act on
   * a run that is not in the roster yet, and returning it would invite a caller to try.
   */
  dispatch: (agent: AgentId, task?: TaskId, prompt?: string) => Promise<void>
  /** Cancel a queued run, or kill a running one. Its row moves to Recent; nothing is removed. */
  stop: (run: RunId) => Promise<void>
  /**
   * Shut the dispatch queue and freeze the children — one run, or the whole project.
   *
   * With no argument this is the **project scope**, which freezes the user's own console session
   * along with every run; see the header for why that makes the resume control's *location* part
   * of the feature rather than a detail of it.
   */
  pause: (run?: RunId) => Promise<void>
  /** `SIGCONT`, reopen the queue, drain it. The mirror of `pause`, same scope rule. */
  resume: (run?: RunId) => Promise<void>
  /**
   * Take a resumed run's stale-turn offer and re-send its last prompt. **Spends a turn.**
   *
   * Takes a run id and never the project: the offer is a suspicion about one child's turn, and
   * a scope that re-sent every prompt in the project would be the automatic re-dispatch this
   * whole mechanism exists to avoid.
   */
  retryTurn: (run: RunId) => Promise<void>
  /** Take the offer off the run and do nothing else. The *leave it* half, which costs nothing. */
  ackStaleTurn: (run: RunId) => Promise<void>
  /**
   * Show a run's transcript in a pane of this window's project console. No new child, ever.
   *
   * Takes the id rather than the `RunView` the panel drew, because the roster this store holds
   * is the only honest source for "does that run still have a session": the view a row was
   * painted from can be a broadcast old by the time it is clicked. The lookup is here and the
   * gesture is in `AgentsPanel/openRun.ts`.
   */
  openPane: (run: RunId) => Promise<void>
  /**
   * Merge a role's `cide/<agent>` branch into the branch this project has checked out.
   *
   * **The one gesture in this store that answers with a value rather than with `void`**, and the
   * only one that touches the user's own working tree. Every other write here is fire-and-refresh
   * because the screen is the answer: the roster moves, and the row says what happened. This one
   * changes nothing the roster carries — a merge does not start, stop or queue a run — so if the
   * outcome were dropped here there would be no surface anywhere saying what it was, and the
   * three answers ("nothing to do", "merged, here is the commit", "refused, here are the paths")
   * are three different next actions.
   *
   * So it is returned and `AgentsPanelHost` reports it. The rejection reaches the caller too, as
   * everywhere else in this file: Rust refuses a detached `HEAD`, a rebase in progress, a role
   * that has never run and a project that is not a repository, each with a sentence.
   *
   * There is deliberately **no refresh afterwards**. The roster is derived from
   * `.cide/config.json`, `.cide/agents/*.md` and the run registry, and a merge moves none of the
   * three; the surfaces that *do* move — the file tree, the Git panel — are driven by the
   * filesystem watcher, which sees the checkout. A refresh here would be a round trip asking a
   * question whose answer cannot have changed.
   */
  integrate: (agent: AgentId) => Promise<AgentIntegration>
}

/**
 * Claimed *before* each call and re-checked after, so an answer for a project the user has since
 * left is dropped rather than painted. The same guard `tasksStore`, `diagnosticsStore` and
 * `gitStatusStore` use, and the same reason: `attach` is called from a render effect and can be
 * re-entered before the previous call lands.
 */
let generation = 0

export const useAgents = create<AgentsStore>((set, get) => ({
  project: null,
  roster: ROSTER_UNKNOWN,
  config: null,
  focusRole: null,

  takeFocusRole: () => {
    const role = get().focusRole
    // Guarded rather than an unconditional `set({ focusRole: null })`: this is called from a
    // render effect on every mount of the settings screen, and zustand notifies its subscribers
    // for a write whether or not the value changed.
    if (role !== null) set({ focusRole: null })
    return role
  },

  attach: async (project) => {
    generation += 1
    /*
     * Cleared to `ROSTER_UNKNOWN` **before the await**, not left standing. The previous
     * project's roles under the new project's name is a *wrong* answer that lasts a frame and
     * reads as real; "nobody has looked" is a true one that lasts one round trip and draws
     * nothing at all. `Roster`'s fourth arm exists for precisely this window — see its doc
     * comment, which argues why `disabled` must not be the placeholder: it is a designed screen
     * that tells the user a feature is off, above a button that writes a file into their
     * repository.
     *
     * The config goes with it, for the same reason and one step further: a `maxConcurrent` from
     * the project the user just left is a number about a different repository.
     */
    set({ project, roster: ROSTER_UNKNOWN, config: null, focusRole: null })
    if (project === null) return
    await get().refresh()
  },

  refresh: async () => {
    const project = get().project
    if (project === null) return
    generation += 1
    const mine = generation

    /*
     * Both go through `pendingCommand` inside `client.ts`, so a build whose backend has no
     * `agents_roster` handler answers `null` rather than rejecting — this is reached from a
     * render effect, and an unhandled rejection out of an effect unmounts the tree under React
     * 19. `null` is *"this build cannot answer"*, which is the same **state** as "the answer is
     * not in yet" and is therefore `ROSTER_UNKNOWN`; it is emphatically not `disabled`, which
     * would claim this project's subagents are off on the strength of a missing command.
     *
     * Asked together rather than in sequence: they are two reads of one directory and a user
     * who opens the panel should not wait for two round trips in series to see one screen.
     */
    const [wireRoster, wireConfig] = await Promise.all([
      agentsApi.roster(project),
      agentsApi.config(project),
    ])
    /*
     * Both guards, in this order and re-checked **after** the await: a newer call has superseded
     * this one, or the user moved to another project while it was in flight. Either alone lets a
     * stale answer through — the generation alone misses a re-attach to the same project id, and
     * the project alone misses two refreshes of the same project racing each other, which is
     * exactly what a broadcast arriving during an `attach` produces.
     */
    if (mine !== generation || get().project !== project) return
    set({
      roster: wireRoster === null ? ROSTER_UNKNOWN : adaptRoster(wireRoster),
      config: wireConfig,
    })
  },

  adopt: (project, wire) => {
    // A broadcast for a project this window is not showing. Every window hears every emit.
    if (get().project !== project) return
    /*
     * The generation is claimed here as well as in `refresh`, and this is the line that makes
     * the guard actually cover the hazard its comment describes: a `refresh` in flight when this
     * broadcast lands would otherwise pass `mine !== generation` and overwrite the *newer*
     * adopted roster with its older answer. Refresh-vs-refresh was covered; broadcast-vs-refresh
     * was not, and fs-driven emits (an agent writing a role file) make that race an everyday one
     * rather than a curiosity.
     */
    generation += 1
    /*
     * Set unconditionally, with no drop rule, and the header says why at length: one in-process
     * writer means the last emit is the newest by construction, so there is nothing to compare
     * a counter against. The store does not fabricate object identity either — a fresh `Roster`
     * per event is correct, because an event that carried the same roster twice is a thing the
     * registry does not do.
     */
    set({ roster: adaptRoster(wire) })
  },

  adoptConfig: (project, config) => {
    // A config for a project this window is not showing. See the interface for why the settings
    // screen writes it and this store only hears about it.
    if (get().project !== project) return
    set({ config })
  },

  enable: async () => {
    const project = get().project
    if (project === null) return
    /*
     * **Awaited, and its rejection deliberately reaches the caller.** This writes
     * `.cide/config.json` into the user's repository — the one gesture in this feature that
     * changes what their next commit contains — and `agents_config_set` refuses it with a
     * sentence when the project is not a git repository. A swallowed failure here is a user
     * pressing the button and being told nothing at all, which is the failure this project has
     * paid for most often. `AgentsPanelHost` puts the reason on screen through `notifyFailure`;
     * this store deliberately does not import `chrome/notices`, so the surfacing stays at the
     * gesture where a reader looking at the click can find it.
     *
     * The answer is the config as it now stands, so it is kept — but the *roster* is what the
     * panel draws, and only Rust knows whether turning the switch on found any role files. So
     * the refresh below is not belt-and-braces: it is the only thing that can move the screen
     * off the `disabled` arm. The broadcast will say the same thing a moment later, and adopting
     * it twice is harmless for `adopt`'s reason.
     */
    const config = await agentsApi.setConfig(project, { enabled: true })
    if (get().project !== project) return
    set({ config })
    await get().refresh()
  },

  configure: async (agent) => {
    const project = get().project
    if (project === null) return
    /*
     * Recorded **before** the await, so the screen finds it already there on the frame it mounts.
     * The other order is a race with nothing to win it: `openTab` resolves once the tab exists,
     * and the settings screen's own effect can run first.
     *
     * `agent ?? null` rather than leaving it: the empty panel's button names no role, and that
     * has to *clear* whatever the last Configure press left behind. A stale id here would open
     * the screen on a role the user did not ask about — quietly, since nothing on that screen
     * would say where the selection came from.
     */
    set({ focusRole: agent ?? null })
    /*
     * Awaited, and the rejection deliberately reaches the caller — `enable`'s argument, and this
     * is the control that most needs it: Configure is drawn on **every** role row whatever the
     * role's state, so it is the one press a user makes when nothing else on the row works, and
     * a silent failure there reads as the whole panel being broken.
     *
     * `'agents'` is `SettingsSection::Agents`, which is already on the wire; the section selects
     * a screen and not a row, which is what `focusRole` is for. No refresh follows: opening a tab
     * changes nothing about the roster.
     */
    await settingsApi.openTab(project, 'agents')
  },

  dispatch: async (agent, task, prompt) => {
    const project = get().project
    if (project === null) return
    /*
     * The two conditional spreads are `exactOptionalPropertyTypes`, not style. ts-rs renders
     * `#[ts(optional)]` as `task?: TaskId`, which under that flag means **absent** and does not
     * admit `undefined` as a value — so `{ task }` with nothing selected is a type error here
     * rather than a `null` arriving at serde from a caller who thought they were sending one.
     * Spreading `{}` is how a field is genuinely left off an object literal.
     *
     * An all-whitespace prompt is dropped rather than sent: `None` means "the task speaks for
     * itself", and a blank string is that same fact spelled as a value — which
     * `cide-agents` would then append to the prompt as an empty line.
     */
    const request: DispatchRequest = {
      project,
      agent,
      ...(task === undefined ? {} : { task }),
      ...(prompt === undefined || prompt.trim() === '' ? {} : { prompt }),
    }
    /*
     * Awaited, unwrapped, and the rejection deliberately reaches the caller — `enable`'s
     * argument, and it is the same one every gesture in this file makes. `agents_dispatch`
     * refuses with a sentence (the project is disabled, the role is at its ceiling, the harness
     * binary went away between the roster and the click), and a swallowed refusal is a Dispatch
     * button that looks broken. `AgentsPanelHost` puts it on screen through `notifyFailure`.
     */
    await agentRunsApi.dispatch(request)
    if (get().project !== project) return
    await get().refresh()
  },

  stop: async (run) => {
    const project = get().project
    if (project === null) return
    await agentRunsApi.stop(project, run)
    if (get().project !== project) return
    await get().refresh()
  },

  /*
   * The four pause gestures, all the same shape as `stop` above and deliberately so: await the
   * command, re-check the project, re-ask. Nothing here is optimistic — the header's second
   * argument applies hardest to these, because the registry is the only thing that knows which
   * sessions the kernel was actually asked to stop, and a row this store drew `paused` on its
   * own would be a claim about a process.
   *
   * Rejections deliberately reach the caller, as everywhere else in this file. Rust refuses each
   * of these with a sentence a user can act on — a run that has not started has no child to
   * freeze, a stale-turn offer that has already been taken cannot be taken twice — and
   * `AgentsPanelHost` puts it on screen through `notifyFailure`.
   *
   * `run` is `undefined` at this seam and `null` on the wire: `client.ts` defaults it, because
   * *the project* is a real scope and not a missing argument.
   */
  pause: async (run) => {
    const project = get().project
    if (project === null) return
    await agentRunsApi.pause(project, run ?? null)
    if (get().project !== project) return
    await get().refresh()
  },

  resume: async (run) => {
    const project = get().project
    if (project === null) return
    await agentRunsApi.resume(project, run ?? null)
    if (get().project !== project) return
    await get().refresh()
  },

  retryTurn: async (run) => {
    const project = get().project
    if (project === null) return
    await agentRunsApi.retryTurn(project, run)
    if (get().project !== project) return
    await get().refresh()
  },

  ackStaleTurn: async (run) => {
    const project = get().project
    if (project === null) return
    await agentRunsApi.ackStaleTurn(project, run)
    if (get().project !== project) return
    await get().refresh()
  },

  openPane: async (run) => {
    const project = get().project
    if (project === null) return
    /*
     * The row the panel drew is not the argument, so the run is looked up here — in the roster
     * this store holds, which is the newest thing this window has. A miss means the run left the
     * roster between the paint and the click, and the honest response is to re-ask rather than to
     * open a pane on a run cide no longer knows about: the refresh is what takes the row off the
     * screen, and there is nothing to tell the user that the screen will not say better.
     */
    const roster = get().roster
    const view = roster.kind === 'ready' ? roster.runs.find((r) => r.run === run) : undefined
    if (view === undefined) {
      await get().refresh()
      return
    }
    /*
     * **No `#[tauri::command]`, and the header says why at length.** This adds a row to the
     * project console with `SplitIntent::Mirror` and lets the split machinery finish it, so the
     * pane arrives with a spawn plan, `TerminalPane` takes its mirror branch, and the host is
     * marked `mirrored` — which is the one thing that stops `closePane` killing the agent's
     * `claude` when the user closes the pane they were reading it in.
     *
     * No `PaneId` is kept. The pane reaches this window on the `cide://workspace-changed` every
     * other pane reaches it on; a store that held the id would be a second record of where a pane
     * is, which is exactly what `store/workspace.ts` refuses to be.
     *
     * Awaited, and its rejection deliberately reaches the caller — `enable`'s and `dispatch`'s
     * argument. `AgentsPanelHost` puts the sentence on screen through `notifyFailure`.
     */
    /*
     * Imported here rather than at the top of the module, and it is not a style choice.
     *
     * `openRun` reaches `layout/paneHosts`, which imports `terminal/xterm`, which imports
     * `@xterm/addon-clipboard` — and that addon touches `self` at module scope. Any SSR bundle
     * that statically reaches this store therefore dies under node with
     * `ReferenceError: self is not defined` before a single assertion runs, which is what
     * `check:openspec-render` hit the moment a panel host read the roster. The same rule
     * `GitDiffView` keeps its tokenizer behind, for the same reason and the same check shape.
     *
     * Free to do here because this is already an async action behind a user gesture: nothing
     * renders while the chunk loads, and the rejection still reaches the caller below.
     */
    const { openRunInPane } = await import('./AgentsPanel/openRun')
    await openRunInPane(view)
    if (get().project !== project) return
    /*
     * The one refresh of the three that does not move a row, because opening a pane changes
     * nothing about the run. It is kept because this is the moment a `canOpen` computed at
     * paint time was acted on: a run that exited in between is corrected on screen here rather
     * than 120 ms later, next to a pane that has just drawn `— exited —`.
     */
    await get().refresh()
  },

  integrate: async (agent) => {
    const project = get().project
    if (project === null) {
      /*
       * Every other gesture here returns `void` and can simply do nothing with no project open,
       * which is unreachable anyway — `AgentsPanelHost` withholds the handler entirely in that
       * state, exactly as it does for Dispatch. This one owes its caller a value, and the honest
       * one is "there was nothing to merge": inventing a `merged` with a fabricated commit, or
       * resolving to `undefined` through a widened return type, would each be a claim about a
       * repository nothing looked at.
       */
      return { kind: 'upToDate' }
    }
    /*
     * Awaited and returned whole, and the rejection deliberately reaches the caller — `enable`'s
     * and `dispatch`'s argument. No refresh: see the interface. No optimism either, and here that
     * word means something stronger than elsewhere in this file — a store that guessed at this
     * outcome would be guessing about the contents of the user's working tree.
     */
    return await agentWorktreeApi.integrate(project, agent)
  },
}))
