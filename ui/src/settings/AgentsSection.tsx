/**
 * Settings → Agents: the subagent roles this project and this user define, as a form.
 *
 * Until now the only way to define a role was to open `.cide/agents/<name>.md` in an editor and
 * get the front-matter grammar right — which is what the user objected to, in those words. This
 * screen is the other door onto the same three files: a list of every definition with Edit and
 * Delete on each row, one role's whole definition in a modal dialog, and `agents_draft` /
 * `agents_save` / `agents_delete` underneath.
 *
 * # Three things this screen does that no other settings section does
 *
 * **It reads the store.** Every other section is a pure function of `Settings` plus a `patch`,
 * because settings are global. A role is not: it belongs to a *project*, and `SectionProps` has
 * no project on it because nothing else here needs one. `KeymapSection` next door already
 * establishes the shape — a section that fetches its own state — and this one takes the project
 * from `useActiveProject`, which is the project whose tab strip is holding this Settings tab.
 *
 * **It writes files, not settings.** `useSettings.patch` sends a `SettingsPatch` fire-and-forget
 * with `.catch(() => {})`, and it can afford to because a `cide://workspace-changed` snapshot
 * follows every settings write: a switch whose write failed flicks back on its own. There is no
 * such snapshot for `.cide/agents/`, so nothing in the window would ever contradict a save that
 * did not happen. Every call here is awaited and every failure is drawn.
 *
 * **It refuses per field.** `AgentSaveOutcome::Rejected` carries one `AgentDraftProblem` per
 * field precisely so the message can sit under the box it is about. `cide_ipc::AgentField`'s doc
 * puts it plainly: "a save that answers 'invalid' leaves the user to find which of eleven fields
 * it meant". So there is no banner. Every one of the eleven fields draws its own messages through
 * [`errorsFor`], and `ui/scripts/check-settings-agents.mjs` asserts that every variant the Rust
 * enum can produce has such a call — a variant with nowhere to show its message is a message the
 * user never sees.
 *
 * # The three decisions this screen had to make
 *
 * **Where the scope choice lives: on the form.** See `scopeChangeWarning` in `./agentsDraft`,
 * which carries the argument in full. In one line: changing scope is a *move between two
 * directories*, `AgentDraft.original` exists so the backend can perform one, and a control in the
 * list would make that move happen on a click with nothing said first. On the form it is a value
 * the user changes, reads a sentence about, and then commits with Save.
 *
 * **What unsaved edits do: they are kept, and they are never written by accident.** Settings
 * elsewhere here is fire-and-forget-on-change, and that is right for a toggle and wrong for a
 * system prompt — it is long, it is the substance of the role, and a stray click costs real
 * writing. So: nothing is sent until Save; every way out of a dirty dialog — Escape, the scrim,
 * Cancel, another role, New role, and the sidebar's Configure — is refused on the spot and turned
 * into one *Discard edits* confirm inside the dialog; and the draft survives a section switch or
 * a tab switch in [`DRAFTS`], a module-level cache outside React, which is why a restored draft
 * reopens the dialog rather than sitting invisibly in a map. A *clean* cached draft is
 * deliberately thrown away instead — see `shouldRestore` — because the file has three other
 * writers and a stale read that is saved reverts whatever landed in between.
 *
 * **The form is a dialog.** The user asked for one, and it is also the shape that makes "which
 * role am I editing" impossible to get wrong. `RoleDialog`'s header carries the three things a
 * form dialog needs that `chrome/ConfirmDestructive` does not — a dismissal that cannot discard
 * silently, initial focus in the form rather than on Cancel, and a focus trap — and says where
 * each one diverges from that component's rules and why.
 *
 * # The sidebar's Configure, honoured
 *
 * `sidebar/agentsStore.ts`'s `configure` records the role that was clicked in `focusRole` and
 * opens this tab. This screen subscribes to it, takes it exactly once through `takeFocusRole` —
 * `layout/spawnPlans.ts`'s rule, and that module's header says why a request that survives a read
 * is a bug — and holds it until its own 2N listing can say which *scope* holds the name, which is
 * the half the store cannot supply. `screenOpening` in `./agentsDraft` owns what happens next,
 * including the case that matters: a request arriving over unsaved writing asks rather than
 * switching.
 *
 * # Why the list costs 2N reads
 *
 * Nothing on the wire says which scope a definition lives in. `AgentDef` is the roster row and
 * its own doc explains that it deliberately carries only what a row can act on; the roster is
 * also *merged*, so a project role and a global role of the same name arrive as one entry. The
 * only honest way to find out which files exist is to ask for each of them, which is what
 * `agents_draft` is for, and it is two asks per name. That is a handful of file reads on a
 * blocking pool behind a deliberate navigation — and it buys the one thing the merged roster
 * cannot show: that the global `qa` the user is looking at is inert because this project has one
 * too.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import {
  agentDefs,
  agents as agentsApi,
  type AgentModels,
  type AgentOverride,
  type OrchestrationConfig,
  type OrchestrationPatch,
  type ProjectId,
  type ProjectOverrides,
} from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { OverlayCard } from '@/overlays/ModalShell'
import { useAgents } from '@/sidebar/agentsStore'
import { useActiveProject, useWorkspace } from '@/store/workspace'
// `Row as SettingRow`: `./agentsDraft` already exports a `Row` type — a row of the *role list* —
// and this one is the settings-form row. Aliasing at the import keeps both names honest.
import {
  ActionButton,
  Group,
  Note,
  NumberField,
  PathReadout,
  Row as SettingRow,
  Select,
  TextArea,
  TextField,
  ToggleRow,
} from './controls'
import {
  AGENT_HUES,
  EFFORT_SUGGESTIONS,
  HARNESSES,
  PERMISSION_MODES,
  SCOPES,
  blankDraft,
  canSave,
  closeRequest,
  effectiveHarness,
  colorOf,
  fileKey,
  fromWire,
  harnessLabel,
  isClaudeScope,
  isDirty,
  localProblems,
  metaExtras,
  modalFor,
  modalTitle,
  modelPlaceholder,
  problemsByField,
  restoredModal,
  rowsFor,
  scopeChangeWarning,
  scopeHint,
  scopeLabel,
  screenOpening,
  shouldRestore,
  titleCase,
  toWire,
  unknownColor,
  withColor,
  withMetaExtras,
  usability,
  usabilityLabel,
  type AgentFieldKey,
  type Draft,
  type Entry,
  type Extra,
  type Modal,
  type Problem,
  type Row,
  type Scope,
  type HarnessName,
} from './agentsDraft'
import { Icon } from '@/icons/Icon'

import styles from './AgentsSection.module.css'

/** Which file the form is on. `null` is "nothing selected"; a create is `original: null` inside. */
interface Selection {
  scope: Scope
  name: string
}

/**
 * Unsaved drafts, per project, outside React.
 *
 * The same device `layout/paneHosts.ts` uses and for a related reason: this component unmounts
 * whenever the user clicks another row in the Settings nav, and React state dies with it. A
 * system prompt is the one value on this screen that is expensive to retype, so an unmount must
 * not be able to destroy one. `shouldRestore` is what keeps this from becoming a staleness bug —
 * only a *dirty* draft comes back; a clean one is dropped and read again from disk.
 *
 * Keyed by project because the form is per project. Not persisted anywhere: a draft is lost when
 * the window closes, which the screen says out loud rather than implying otherwise.
 */
const DRAFTS = new Map<string, { selection: Selection | null; saved: Draft | null; draft: Draft }>()

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(' ')

export function AgentsSection() {
  const project = useActiveProject()
  const projectId = project?.id ?? null
  if (projectId === null) {
    return (
      <Note title="No project is open">
        Roles are defined per project — <code>.cide/agents/</code> hangs off a project root — so
        this screen has nothing to edit until one is open. Global roles are edited here too, but
        they are still saved through the open project.
      </Note>
    )
  }
  // Keyed on the project, so switching projects rebuilds the whole screen rather than leaving one
  // project's draft sitting over another project's list.
  return <AgentsEditor key={projectId} project={projectId} />
}

function AgentsEditor({ project }: { project: ProjectId }) {
  /** The definition files that exist, or `null` while the listing is in flight. */
  const [entries, setEntries] = useState<Entry[] | null>(null)
  /** What the roster said, when it said something other than "here are the roles". */
  const [rosterNote, setRosterNote] = useState<{ title: string; body: string } | null>(null)
  const [listError, setListError] = useState<string | null>(null)

  const cached = DRAFTS.get(project) ?? null
  const restore = shouldRestore(cached) ? cached : null
  const [selection, setSelection] = useState<Selection | null>(restore?.selection ?? null)
  const [saved, setSaved] = useState<Draft | null>(restore?.saved ?? null)
  const [draft, setDraft] = useState<Draft | null>(restore?.draft ?? null)
  /**
   * The dialog, or `null` for closed.
   *
   * Initialised from [`restoredModal`], **not** from `null`: a draft that survived the unmount is
   * by construction a dirty one, and coming back to a closed dialog would leave that writing
   * alive in `DRAFTS` with nothing on screen pointing at it. That function's doc carries the
   * argument.
   */
  const [modal, setModal] = useState<Modal>(() => restoredModal(cached))

  /** Rust's refusals from the last save. Cleared per field as that field is edited. */
  const [problems, setProblems] = useState<Problem[]>([])
  /** Which fields the user has been in, so a blank new form is not red before it is touched. */
  const [touched, setTouched] = useState<ReadonlySet<AgentFieldKey>>(() => new Set())
  const [attempted, setAttempted] = useState(false)
  const [busy, setBusy] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [savedPath, setSavedPath] = useState<string | null>(null)
  /** Why a role would not open. Drawn beside the list, because the dialog never appeared. */
  const [openError, setOpenError] = useState<string | null>(null)
  /**
   * Which row's Delete is armed, or `null`.
   *
   * A *file* rather than a boolean now that Delete is a list action: two rows can share a name
   * across scopes, so `scope:name` is the only identity that answers "is this the row I armed".
   * Second-click confirms, the way `AgentsPanel`'s Integrate and `TasksPanel`'s Delete do.
   */
  const [armedDelete, setArmedDelete] = useState<Selection | null>(null)
  /**
   * What the user asked to do while the dialog held unsaved writing — the dialog's discard
   * confirm, and the one thing that can end a dirty draft.
   *
   * Three shapes because there are three ways out of a dirty form: open another role, start a new
   * one, or simply leave. All three are refused on the spot and turned into this, so there is
   * exactly one place in the screen that can throw a system prompt away and it is a place with a
   * name. It moved off the list rows and into the dialog because the rows are behind a scrim
   * while the dialog is up and can no longer draw anything.
   */
  const [pendingLeave, setPendingLeave] = useState<Selection | 'new' | 'close' | null>(null)

  /**
   * This project's `.cide/config.json` as it stands, or `null` when nobody has an answer.
   *
   * `null` is *"not read yet, or this build has no `agents_config_get`"* — `agents.config` goes
   * through `pendingCommand` and answers `null` rather than rejecting — and it draws **nothing**
   * rather than a default. A row showing `2` on a project whose file says `6` is a wrong number
   * that looks exactly like a right one, and the user's next keystroke would write it back.
   *
   * Read here rather than taken from `sidebar/agentsStore.ts`, which also holds one, for this
   * screen's freshness rule: the store loads its copy when the sidebar attaches and this file is
   * *committed*, so a teammate's commit or a `git checkout` can move it under a running app. The
   * store's copy is kept in step the other way round — through `adoptConfig`, after a write.
   */
  const [config, setConfig] = useState<OrchestrationConfig | null>(null)
  /**
   * Why the config could not be read, or could not be written. Drawn under the row it is about.
   *
   * Which of the two is carried rather than inferred: a *write* failure leaves the last good
   * config on screen and a *read* failure leaves nothing, so a heading derived from `config`
   * being null would be right until somebody's second gesture and wrong from then on.
   */
  const [configError, setConfigError] = useState<{ what: 'read' | 'write'; text: string } | null>(
    null,
  )

  /**
   * This project's **local, uncommitted** role redirections. (M45)
   *
   * Held beside `config` and drawn in the same group, but they are not the same kind of thing and
   * the screen has to say so: `config` writes `.cide/config.json`, which the repository will
   * contain and a teammate will review; this writes the profile's own
   * `agent-overrides.json`, which nobody else ever sees. That difference is the whole reason the
   * layer exists — see `cide_ipc::overrides`' header — and a row that did not say which was
   * which would be the lie `OrchestrationConfig`'s doc warns about from the other direction.
   *
   * `null` is a build with no such handler; an *absent* file is an ordinary empty answer.
   */
  const [overrides, setOverrides] = useState<ProjectOverrides | null>(null)
  const [overrideError, setOverrideError] = useState<string | null>(null)
  /**
   * The pools a row may name, from **global** settings.
   *
   * The split this whole feature rests on: a pool's providers and keys are the person's and live
   * in `workspace.json`; which pool a role uses is the project's and lives in the override file.
   * So the menu is drawn from one and written to the other.
   */
  const storedPools = useWorkspace((s) => s.boot?.workspace.settings.llm.pools ?? null)
  const poolNames = useMemo(
    () => (storedPools ?? []).map((pool) => pool.name).filter((name) => name !== ''),
    [storedPools],
  )

  const dirty = draft !== null && isDirty(saved, draft)

  /* --------------------------------------------------------------- the sidebar's Configure */

  /**
   * The role `sidebar/agentsStore.ts`'s `configure` asked this screen to open, or `null`.
   *
   * Subscribed to rather than read once, because the Settings tab is frequently **already open
   * on this section** when Configure is pressed: `tab_open_settings` then focuses the existing
   * tab, this component never remounts, and a mount-only read would miss every such press — the
   * most likely press there is, since the panel and the tab are one click apart.
   *
   * The selector answers a `string | null` and builds nothing, which is what `check:selectors`
   * is about: a selector returning a fresh array or object re-renders for ever.
   *
   * Guarded on the project. The store clears `focusRole` on `attach`, so a request already
   * belongs to the attached project — but the *settings* screen takes its project from
   * `useActiveProject`, and in a second window those two can name different projects. A request
   * meant for one project must not open a role in another.
   */
  const focusRole = useAgents((s) => (s.project === project ? s.focusRole : null))
  /**
   * A fingerprint of the roster this project currently has, so the listing below can follow the
   * disk instead of freezing at whatever was there when this screen mounted.
   *
   * # Why this screen needs it when the panel does not
   *
   * The Agents panel renders straight out of the store, so `cide://agents-changed` moves it for
   * free. This screen does not: its list is `entries`, built by probing `agents_draft` once per
   * scope per name, because a **shadowed** definition is one the roster has already merged away
   * and only a probe can find. That probe ran on mount and never again — so a role written while
   * Settings was open (by Claude Code's own `/agents`, by a teammate's `git pull`, by an
   * orchestrator's Write tool) appeared in the sidebar and was simply absent here, which reads as
   * cide not supporting the thing rather than as a stale screen.
   *
   * **Selected, then derived — never built inside the selector.** `check:selectors` refuses a
   * selector that constructs a value, because a fresh reference on every store read re-renders
   * for ever and ends at *Maximum update depth exceeded*, which unmounts the whole window rather
   * than this component. That the fingerprint below happens to end in a `join` and is therefore a
   * string does not save it: the rule is syntactic on purpose, since the day somebody drops the
   * `join` there is no failure to see. So the selector returns the **stored** roster — one
   * reference, replaced only when `adopt` runs — and the string is derived here.
   *
   * The scope is in the key as well as the id because a *move* between scopes changes which file
   * the list must open while leaving the set of names identical.
   */
  const projectRoster = useAgents((s) => (s.project === project ? s.roster : null))
  const rosterMark = useMemo(() => {
    if (projectRoster === null) return ''
    if (projectRoster.kind !== 'ready') return projectRoster.kind
    return projectRoster.agents.map((def) => `${def.scope}:${def.id}`).join('\n')
  }, [projectRoster])
  /** The request, taken out of the store and held here until the listing can resolve it. */
  const [focusRequest, setFocusRequest] = useState<string | null>(null)
  /** A request the listing could not match, kept so the screen can say so rather than shrug. */
  const [focusMiss, setFocusMiss] = useState<string | null>(null)

  /*
   * Taken, once, the moment it appears — `takeFocusRole` clears it in the store.
   *
   * `layout/spawnPlans.ts`'s `takeSpawnPlan` is the precedent and the reason: React 19's
   * StrictMode mounts effects twice in development, so a value left in place is applied twice.
   * Here the second application is not a duplicate fork but a *later* one — the request would
   * still be sitting in the store on the next mount of this screen (another section, another
   * tab, a relaunch that restores the Settings tab) and would jump the form to a role the user
   * has not asked about since, discarding whatever they had open to do it.
   *
   * Moving it into local state rather than acting on it here is what lets it survive the frames
   * before the file listing lands, without leaving the store's copy readable by anyone else.
   */
  useEffect(() => {
    if (focusRole === null) return
    const taken = useAgents.getState().takeFocusRole()
    if (taken !== null) setFocusRequest(taken)
  }, [focusRole])

  // The cache is written on every change rather than on unmount: an unmount has no hook that
  // runs late enough to read fresh state without a ref, and a ref of the whole form is one more
  // thing to keep in step. Writing through is cheap and cannot go stale.
  useEffect(() => {
    if (draft === null) DRAFTS.delete(project)
    else DRAFTS.set(project, { selection, saved, draft })
  }, [project, selection, saved, draft])

  /**
   * Every definition file, found by asking for each of them.
   *
   * `allSettled` and not `all`: a rejection here is the ordinary answer — it is how "there is no
   * global role by this name" arrives — so a combinator that fails the batch on the first
   * rejection would report an empty list for every project that has any role at all.
   */
  const listRoles = useCallback(async (): Promise<Entry[]> => {
    const roster = await agentsApi.roster(project)
    if (roster === null) {
      setRosterNote({
        title: 'This build cannot list roles',
        body: 'The agents_roster command did not answer. The form below still works — a save writes the file directly — but nothing here can show you what is already defined.',
      })
      return []
    }
    if (roster.kind === 'disabled') {
      setRosterNote({
        title: 'Subagents are switched off for this project',
        body: `${roster.hint} Roles can still be written from here — defining them is what you do before turning orchestration on — but cide cannot list the ones that already exist while the switch is off.`,
      })
      return []
    }
    if (roster.kind === 'empty') {
      setRosterNote({
        title: 'No roles yet',
        body: `Nothing in .cide/agents/, in your global roles, or in this project's or your own .claude/agents/. New role writes the first one; ${roster.configPath} is where the project's own switches live.`,
      })
      return []
    }
    setRosterNote(null)

    const probes = roster.agents.flatMap((def) =>
      SCOPES.map(async (scope): Promise<Entry | null> => {
        try {
          const wire = await agentDefs.draft(project, def.id, scope)
          return {
            scope,
            name: def.id,
            label: wire.label ?? titleCase(def.id),
            // The **file's** harness, from the draft, and not `def.harness`. `AgentDef.harness`
            // is a `Harness` with no unset arm — the roster has already resolved the project
            // default — so it cannot say whether this file named one, which is the state the
            // row and the form both have to draw.
            harness: wire.harness ?? null,
            // The **roster's** answer, from `def`, because a draft carries no such field: it is
            // computed across the harness's binary, the prompt and the config, and only the
            // roster does that work. It is about the merged role, which is why `rowsFor` refuses
            // to leave it on a shadowed row.
            unavailable: def.unavailable ?? null,
          }
        } catch {
          // No such file in this scope. Not an error: it is half of the answer this loop exists
          // to produce, and the roster's merge is exactly what hides it.
          return null
        }
      }),
    )
    const found = await Promise.all(probes)
    return found.filter((entry): entry is Entry => entry !== null)
  }, [project])

  const refresh = useCallback(() => {
    setListError(null)
    void listRoles()
      .then(setEntries)
      .catch((error: unknown) => {
        setEntries([])
        setListError(String(error))
      })
  }, [listRoles])

  /*
   * On mount, and again whenever the roster this project has actually changes.
   *
   * `refresh` re-reads through `agents_roster` and the probes rather than adopting the store's
   * copy, which is the same freshness argument `agents_draft` makes one layer down: this screen
   * *writes* these files, and a list assembled from a snapshot is a list that can save over
   * whatever the user's editor did in between. `rosterMark` is only the signal to go and look.
   *
   * No loop: nothing in `refresh` writes to the agents store, so the mark cannot move because the
   * listing ran.
   */
  useEffect(refresh, [refresh, rosterMark])

  /*
   * The project's own switches, on the same signal as the listing.
   *
   * `rosterMark` is in the deps because the two move together: the gesture that most often
   * changes this file is the panel's *Enable subagents*, and that is also what turns the roster
   * from `disabled` into a list of roles.
   *
   * `live` rather than an AbortController, because there is nothing to abort — the round trip
   * will finish either way and all this guards is a `setState` on an unmounted component, which
   * a project switch produces on every navigation (this screen is keyed on the project).
   */
  useEffect(() => {
    let live = true
    void agentsApi
      .config(project)
      .then((next) => {
        if (!live) return
        setConfig(next)
        setConfigError(null)
      })
      .catch((error: unknown) => {
        if (live) setConfigError({ what: 'read', text: errorText(error) })
      })
    return () => {
      live = false
    }
  }, [project, rosterMark])

  // The overrides, on the same key as the config above: a roster change can add the role a
  // per-role row is about.
  useEffect(() => {
    let live = true
    void agentDefs
      .overrides(project)
      .then((next) => {
        if (!live) return
        setOverrides(next)
        setOverrideError(null)
      })
      .catch((error: unknown) => {
        if (live) setOverrideError(errorText(error))
      })
    return () => {
      live = false
    }
  }, [project, rosterMark])


  /* ------------------------------------------------------- what the Model box may offer ----- */

  /**
   * Which harness the Model menu is for.
   *
   * `effectiveHarness` and not `draft.harness`, because that field is nullable: "project
   * default" is a real state, so the answer needs `.cide/config.json` — which is loaded here and
   * nowhere below. Resolved once, here, so the probe and the control cannot disagree about which
   * CLI is being asked.
   *
   * A closed dialog resolves to `null` and asks nothing: this forks a process, and a settings
   * screen nobody has opened a role on should not be running `opencode models`.
   */
  const modelHarness = useMemo(
    () => (draft === null ? null : effectiveHarness(draft, config?.harness ?? null)),
    [draft, config],
  )

  /**
   * One answer per harness, kept for as long as the screen is open.
   *
   * Keyed by harness rather than held as a single value because flipping the Harness dropdown
   * back and forth is an ordinary thing to do while deciding, and re-forking `opencode models`
   * on every flip would be a spinner on a question already answered. The screen is keyed on the
   * project, so a project switch remounts and drops the cache with it — which is right, since
   * the probe runs in the project's directory.
   */
  const [modelAnswers, setModelAnswers] = useState<Partial<Record<HarnessName, AgentModels>>>({})
  const [modelsPending, setModelsPending] = useState<HarnessName | null>(null)

  useEffect(() => {
    if (modelHarness === null) return
    // The cache check is also the loop guard: this effect depends on `modelAnswers`, which it
    // writes, so without it every answer would start the next probe.
    if (modelAnswers[modelHarness] !== undefined) return
    let live = true
    setModelsPending(modelHarness)
    // No `.catch`: `agentDefs.models` goes through `pendingCommand`, so a build without the
    // handler resolves to `null` rather than rejecting — and `null` is a state this field draws.
    void agentDefs.models(project, modelHarness).then((answer) => {
      if (!live) return
      // A `null` is cached as an empty answer *for this harness*, so a degraded build asks once
      // and then draws a plain text box, rather than forking on every keystroke that re-renders.
      setModelAnswers((prev) => ({
        ...prev,
        [modelHarness]: answer ?? { harness: modelHarness, models: [], problem: null },
      }))
      setModelsPending((pending) => (pending === modelHarness ? null : pending))
    })
    return () => {
      live = false
    }
  }, [project, modelHarness, modelAnswers])

  /**
   * Write one project switch into `.cide/config.json`, creating the file if it is not there.
   *
   * Three things this does that the settings rows next door do not, and all three are the same
   * fact wearing different hats — **this is a file in the user's repository**, not a `Settings`
   * field:
   *
   * * It is **awaited and its failure is drawn**. `useSettings.patch` can be fire-and-forget
   *   because a `cide://workspace-changed` snapshot follows every write and a switch whose write
   *   failed flicks back on its own; nothing contradicts a per-project write that did not happen.
   * * The answer is **adopted, not assumed**. `agents_config_set` clamps — `maxConcurrent` of 0
   *   lands as 1 — so the number the field must show afterwards is the one Rust wrote, never the
   *   one that was typed.
   * * `errorText`, never `String(error)`: a refusal arrives as a tagged `CoreError`, so
   *   `String()` of it is `[object Object]` and the sentence Rust composed is lost.
   */
  /**
   * Write the whole override table back.
   *
   * Whole-table rather than per row, for the command's stated reason, and **not** fire-and-forget:
   * an override that silently failed to save is a role that runs somewhere other than where the
   * screen says it does, which is the one failure this surface must not have.
   */
  const writeOverrides = useCallback(
    (next: ProjectOverrides) => {
      setOverrides(next)
      setOverrideError(null)
      void agentDefs
        .setOverrides(project, next)
        .then(setOverrides)
        .catch((error: unknown) => setOverrideError(errorText(error)))
    },
    [project],
  )

  /**
   * Every write to `.cide/config.json` this screen makes.
   *
   * `Partial<OrchestrationPatch>` rather than a hand-listed union since M79: there are six
   * settable keys now, and a signature enumerating them a second time is a second place for the
   * list to go stale — which would fail as a *type* error at the call site rather than as the
   * silent one, but would still be two lists.
   */
  const patchConfig = useCallback(
    (patch: OrchestrationPatch) => {
      setConfigError(null)
      void agentsApi
        .setConfig(project, patch)
        .then((next) => {
          setConfig(next)
          // The other holder of this value, kept in step rather than left to go stale. Guarded
          // inside the store on the project, because that store follows the sidebar and this
          // screen follows the active tab, and in a second window they can differ.
          useAgents.getState().adoptConfig(project, next)
        })
        .catch((error: unknown) => setConfigError({ what: 'write', text: errorText(error) }))
    },
    [project],
  )

  const rows = useMemo(() => rowsFor(entries ?? []), [entries])
  const taken = useMemo(
    () => new Set((entries ?? []).map((entry) => fileKey(entry.scope, entry.name))),
    [entries],
  )

  /* ------------------------------------------------------------------ opening and leaving */

  const load = useCallback(
    (target: Selection) => {
      setBusy(true)
      setSaveError(null)
      setSavedPath(null)
      setOpenError(null)
      void agentDefs
        .draft(project, target.name, target.scope)
        .then((wire) => {
          const next = fromWire(wire)
          setSelection(target)
          setSaved(next)
          setDraft(next)
          // Derived from the draft rather than asserted from `target`: `read_draft` fills
          // `original` in, and `modalFor` reads the same field `agents_save` reads to tell an
          // edit from a create. One fact, one reader.
          setModal(modalFor(next))
          setProblems([])
          setTouched(new Set())
          setAttempted(false)
          setArmedDelete(null)
          setPendingLeave(null)
        })
        // Not `setSaveError`: that one is drawn *inside* the dialog, and a load that failed is
        // precisely the case where no dialog opened. A sentence with nowhere to appear is the
        // failure this screen's per-field errors exist to make unrepresentable.
        .catch((error: unknown) => setOpenError(String(error)))
        .finally(() => setBusy(false))
    },
    [project],
  )

  /**
   * Open a role, or refuse to and arm a confirm instead.
   *
   * The dirty guard is here rather than on each row so that there is exactly one place that can
   * throw away a system prompt, and it is a place with a name.
   */
  const open = useCallback(
    (target: Selection) => {
      // Opening the role that is already in the dialog is a no-op rather than a re-read. A
      // re-read would be the one gesture on this screen that throws a system prompt away without
      // asking, and it would be the gesture that looks least like doing anything.
      if (
        modal !== null &&
        selection !== null &&
        selection.name === target.name &&
        selection.scope === target.scope
      ) {
        return
      }
      if (dirty) {
        setPendingLeave(target)
        return
      }
      setPendingLeave(null)
      load(target)
    },
    [dirty, modal, selection, load],
  )

  /**
   * Throw the form away and start a blank one.
   *
   * Split from [`startNew`] rather than folded into it behind a boolean, the way
   * `AgentsPanel`'s `onIntegrateArm` / `onIntegrate` pair is split: one handler taking "yes,
   * really discard" lets the control that was only meant to *ask* pass the answer.
   */
  const forceNew = useCallback(() => {
    setPendingLeave(null)
    setSelection(null)
    setSaved(null)
    setDraft(blankDraft('project'))
    setModal({ kind: 'new' })
    setProblems([])
    setTouched(new Set())
    setAttempted(false)
    setArmedDelete(null)
    setSavedPath(null)
    setSaveError(null)
    setOpenError(null)
  }, [])

  const startNew = useCallback(() => {
    if (dirty) {
      setPendingLeave('new')
      return
    }
    forceNew()
  }, [dirty, forceNew])

  /** Shut the dialog and drop the draft. Only ever reached with nothing at stake — see below. */
  const forceClose = useCallback(() => {
    setPendingLeave(null)
    setModal(null)
    setDraft(null)
    setSaved(null)
    setProblems([])
    setTouched(new Set())
    setAttempted(false)
    setSaveError(null)
  }, [])

  /**
   * Escape, the scrim, and the dialog's Cancel button, all through one funnel.
   *
   * `closeRequest` decides, and its doc carries the argument in full: clean closes, dirty asks
   * and the dialog stays up. The three dismissals share this handler on purpose — a scrim click
   * that discarded writing an Escape would have asked about is the same dialog behaving two
   * ways.
   */
  const requestClose = useCallback(() => {
    if (closeRequest(dirty) === 'ask') {
      setPendingLeave('close')
      return
    }
    forceClose()
  }, [dirty, forceClose])

  /** The answer to the discard confirm: do the thing that was refused, and lose the edits. */
  const confirmLeave = useCallback(() => {
    const pending = pendingLeave
    setPendingLeave(null)
    if (pending === null) return
    if (pending === 'close') forceClose()
    else if (pending === 'new') forceNew()
    else load(pending)
  }, [pendingLeave, forceClose, forceNew, load])

  /* ------------------------------------------------- honouring the Configure that was pressed */

  /*
   * One decision function, one effect. `screenOpening` owns the ordering — a focus request
   * outranks the dialog that is up, and neither outranks unsaved writing — so this switch is
   * only the doing.
   *
   * `wait` is the arm that makes the feature work at all: `configure` opens the tab and this
   * screen mounts in the same frame, so the request is nearly always here before the 2N file
   * probes come back. Holding is why `focusRequest` is state rather than a value consumed on the
   * spot; the request is cleared the moment it is *answered*, in every other arm, so it can never
   * fire twice.
   */
  useEffect(() => {
    const opening = screenOpening(focusRequest, entries === null ? null : rows, dirty, modal)
    if (opening.kind === 'focus') {
      setFocusRequest(null)
      setFocusMiss(null)
      load(opening.file)
    } else if (opening.kind === 'confirm') {
      setFocusRequest(null)
      setFocusMiss(null)
      setPendingLeave(opening.file)
    } else if (opening.kind === 'unknown') {
      setFocusRequest(null)
      setFocusMiss(opening.name)
    }
  }, [focusRequest, entries, rows, dirty, modal, load])

  /* ------------------------------------------------------------------------------ editing */

  const edit = useCallback((field: AgentFieldKey, patch: Partial<Draft>) => {
    setDraft((current) => (current === null ? current : { ...current, ...patch }))
    setTouched((current) => {
      if (current.has(field)) return current
      const next = new Set(current)
      next.add(field)
      return next
    })
    // Rust's refusal for this field described the value that was sent, not the one being typed.
    // The other fields' refusals stay: a form that cleared them all on one keystroke would make
    // the user press Save again just to see the list it had already been given.
    setProblems((current) => current.filter((problem) => problem.field !== field))
  }, [])

  const save = useCallback(() => {
    if (draft === null) return
    setAttempted(true)
    if (!canSave(draft)) return
    setBusy(true)
    setSaveError(null)
    void agentDefs
      .save(project, toWire(draft))
      .then((outcome) => {
        if (outcome.kind === 'rejected') {
          setProblems(outcome.problems)
          setSavedPath(null)
          return
        }
        /*
         * A write closes the dialog. The role is on disk, the draft is no longer *un*saved, and
         * a modal left standing over its own success is a modal the user has to dismiss to see
         * whether the list changed — which is the one thing they wanted to see.
         *
         * A **rejection** does not close, and that asymmetry is the point of the branch above:
         * `AgentSaveOutcome::Rejected` carries one sentence per field, and every one of them
         * belongs under a box that only exists inside this dialog.
         */
        setProblems([])
        setSelection({ scope: draft.scope, name: draft.name.trim() })
        setSaved(null)
        setDraft(null)
        setModal(null)
        setTouched(new Set())
        setSavedPath(outcome.path)
        setAttempted(false)
        setPendingLeave(null)
        refresh()
      })
      .catch((error: unknown) => setSaveError(String(error)))
      .finally(() => setBusy(false))
  }, [draft, project, refresh])

  /**
   * Delete one definition file, from its row.
   *
   * It takes the file rather than reading `selection`, because Delete is a **list** action now:
   * the row the user armed is the row that is deleted, whether or not anything is open. The
   * dialog lost its own Delete in the same move — a form whose footer offers *Save* and *destroy
   * the thing you are editing* side by side is a footer where the wrong one is one slip away, and
   * the row is where the user is already looking when they decide a role should go.
   */
  const remove = useCallback(
    (target: Selection) => {
      setBusy(true)
      setSaveError(null)
      setOpenError(null)
      void agentDefs
        .delete(project, target.name, target.scope)
        .then(() => {
          setArmedDelete(null)
          setSavedPath(null)
          // Only the *open* role's form goes away with the file. Deleting some other row while a
          // draft is up must not close the dialog over it — that would be the discard this whole
          // screen refuses to make silently.
          if (selection !== null && selection.scope === target.scope && selection.name === target.name) {
            setSelection(null)
            setSaved(null)
            setDraft(null)
            setModal(null)
            setProblems([])
            setPendingLeave(null)
            DRAFTS.delete(project)
          }
          refresh()
        })
        .catch((error: unknown) => setOpenError(String(error)))
        .finally(() => setBusy(false))
    },
    [project, selection, refresh],
  )

  /* ------------------------------------------------------------------------------ drawing */

  const byField = useMemo(() => {
    const local = draft === null ? [] : localProblems(draft)
    const shown = local.filter((problem) => attempted || touched.has(problem.field))
    return problemsByField([...problems, ...shown])
  }, [problems, draft, attempted, touched])

  /** Every message that belongs under one box. The only way a refusal reaches the screen. */
  const errorsFor = (field: AgentFieldKey): string[] => byField.get(field) ?? []

  const moveWarning = draft === null ? null : scopeChangeWarning(draft, taken)

  return (
    <>
      {/*
        * The project's own switches — the only thing on this screen that is not a role file.
        *
        * # Why it is drawn here at all, when `OrchestrationConfig`'s doc says this config gets no
        * settings row
        *
        * That doc's rule is about `SECTIONS` and `useSettings.patch`: nothing about a *project*
        * may ride `Workspace.settings`, so a row that looked like every other row and meant
        * something different would be a lie about where the value lives. It then says where these
        * switches belong instead, in as many words — "it is where the switches in *this* struct
        * will eventually be drawn too, and when they are they will still be a per-project file
        * write and still not a `SettingsPatch`". This is that, and it holds to both halves:
        * `agents_config_set` writes `.cide/config.json`, the hint says so, and no patch is sent.
        *
        * # Nothing at all when the config is unknown
        *
        * `agents.config` answers `null` on a build with no such handler, and a number is exactly
        * the kind of value that must not be guessed: `2` is also the default, so a guessed row on
        * a project whose file says `6` is indistinguishable from a correct one — and the user's
        * next keystroke writes the guess back over what was there.
        */}
      {(config !== null || configError !== null) && (
        <Group title="This project">
          {config !== null && (
            <SettingRow
              label="Concurrent runs"
              hint={
                'How many subagent runs may be live across this project at once, whatever a ' +
                "role's own max-concurrent allows. Written to .cide/config.json, which your " +
                'repository will then contain.'
              }
              control={
                <NumberField
                  label="Concurrent runs"
                  value={config.maxConcurrent}
                  min={1}
                  /*
                   * A ceiling, not a policy: the value is a `u16` and Rust only clamps the bottom
                   * of it (0 would be a project that can never dispatch). 64 is far above any
                   * fan-out a person means and well above anything a hand-edited file is likely
                   * to hold — which matters, because `NumberField` commits on blur, so a maximum
                   * *below* a hand-written number would quietly lower it the first time the field
                   * was tabbed through.
                   */
                  max={64}
                  onChange={(next) => patchConfig({ maxConcurrent: next })}
                />
              }
            />
          )}
          {config !== null && (
            <ToggleRow
              label="Review finished runs in a new tab"
              hint={
                'When a subagent hands its turn back, open a fresh Claude tab to check the ' +
                'work instead of typing a line into this project\u2019s console. The new ' +
                'conversation starts empty, so it reads the task and the diff with nothing ' +
                'else in its context \u2014 and the tab stays until you close it, which ends ' +
                'the Claude in it. Off, the console is told instead, exactly as before.'
              }
              checked={config.finishInNewTab}
              onChange={(next) => patchConfig({ finishInNewTab: next })}
            />
          )}
          {config !== null && (
            <ToggleRow
              label="Wake this project when it goes quiet"
              hint={
                'With subagents on and tasks still open, if nothing has been running for the ' +
                'period below, open a Claude in plan mode and ask it to check what was done ' +
                'and put the next tasks on the roles. It never fires while a run is live, ' +
                'while a Claude pane is working, or while agents are paused.'
              }
              checked={config.autoSpin}
              onChange={(next) => patchConfig({ autoSpin: next })}
            />
          )}
          {config !== null && config.autoSpin && (
            <>
              <SettingRow
                label="Quiet for"
                hint={
                  'Minutes of nothing happening before it fires. The clock restarts whenever ' +
                  'the project looks busy, so this is a dwell and not an interval.'
                }
                control={
                  <NumberField
                    label="Quiet for"
                    /*
                     * Seconds on the wire, minutes in the box. The file holds seconds because
                     * every other duration in it does (`stopGraceSecs`), and the box holds
                     * minutes because nobody means "nine hundred". `Math.round` on the way in
                     * so a hand-written 90 seconds draws as 2 rather than as 1.5, which
                     * `NumberField` would commit back as 1.
                     */
                    value={Math.round(config.autoSpinAfterSecs / 60)}
                    min={1}
                    max={1440}
                    onChange={(next) =>
                      patchConfig({ autoSpinAfterSecs: Math.round(next) * 60 })
                    }
                  />
                }
              />
              {/*
                * A `TextArea` and not a `TextField`, which reverses what M79 first shipped.
                *
                * The argument for one line was that the prompt is *typed at a terminal*, where a
                * newline is a second Enter — so a control that could not hold one was the
                * constraint expressed in the form. That reasoning was sound about the file and
                * wrong about the person: the default prompt is some five hundred characters, and
                * a 26px box is the wrong shape to read it in, let alone rewrite it. Reported as
                * exactly that.
                *
                * Nothing about the constraint moved, because it was never this control's to
                * keep: `AgentsConfig::apply` flattens the string before it reaches
                * `.cide/config.json`, which is why the flattening went *there* rather than at
                * the spawn. So the box can hold whatever somebody types, the file holds one
                * line, and the box shows that line back after the round trip — which is also
                * what makes the hint below true rather than a warning nobody can act on.
                */}
              <TextArea
                label="What to tell it"
                hint={
                  'The first prompt for the new Claude. Leave it empty to use the one cide ' +
                  'ships, which tells it to survey the board and the git log first, judge ' +
                  'whether the work is going the right way, and only then assign. Line breaks ' +
                  'become spaces when it is saved \u2014 it is typed into a terminal, where a ' +
                  'newline would submit half a sentence.'
                }
                value={config.autoSpinPrompt}
                placeholder="Use cide's own prompt"
                onCommit={(next) => patchConfig({ autoSpinPrompt: next })}
              />
              <ToggleRow
                label="Accept its plan automatically"
                hint={
                  'Plan mode ends at an approval that a person would normally give, and there ' +
                  'is nobody there. cide reads that prompt and answers it \u2014 for this one ' +
                  'session, only when it recognises the plan question, and only the option that ' +
                  'says yes. Any other prompt is left alone. Off, the tab waits at its plan ' +
                  'with the pane marked and you approve it yourself.'
                }
                checked={config.autoSpinAcceptPlan}
                onChange={(next) => patchConfig({ autoSpinAcceptPlan: next })}
              />
            </>
          )}
          {configError !== null && (
            <Note
              title={
                configError.what === 'read'
                  ? "This project's switches could not be read"
                  : 'That switch could not be written'
              }
              tone="warn"
            >
              {configError.text}
            </Note>
          )}
        </Group>
      )}

      <Group title="Roles">
        <div className={styles.split}>
          <RoleList
            rows={rows}
            loading={entries === null}
            selection={selection}
            armedDelete={armedDelete}
            busy={busy}
            onEdit={open}
            onArmDelete={setArmedDelete}
            onDisarmDelete={() => setArmedDelete(null)}
            onDelete={remove}
          />
          <div className={styles.listActions}>
            <ActionButton label="New role" onClick={startNew} />
          </div>
        </div>
        {rosterNote !== null && <Note title={rosterNote.title}>{rosterNote.body}</Note>}
        {listError !== null && (
          <Note title="The role list could not be read" tone="warn">
            {listError}
          </Note>
        )}
        {openError !== null && (
          <Note title="That role could not be opened" tone="warn">
            {openError}
          </Note>
        )}
        {/*
          * A Configure press whose role has no definition file this screen can read.
          *
          * Said rather than swallowed: `AgentDef.unavailable`'s doc makes the case one layer
          * down — "we could not tell" and "it is not there" must stay distinguishable — and a
          * Configure that opened the screen and then did nothing visible is exactly the silently
          * inert control this project keeps a check script to prevent.
          */}
        {focusMiss !== null && (
          <Note title={`No definition file for “${focusMiss}”`} tone="warn">
            The roster lists this role, but neither <code>.cide/agents/{focusMiss}.md</code> nor
            your global copy could be read just now — it may have been deleted, or it may be
            declared somewhere other than a definition file. New role writes a fresh one.
          </Note>
        )}
        {/* Where the last write landed. It sits with the list rather than in the dialog because
            the dialog is gone by the time it is true: a save closes it. */}
        {savedPath !== null && (
          <>
            <p className={styles.wrote}>Written. This role now lives in:</p>
            <PathReadout path={savedPath} />
          </>
        )}
      </Group>

      {/*
        * The local overrides, **after** the roles rather than beside the project's switches, and
        * that order is the argument: an override redirects a role, so it can only be read once
        * you know what the roles are. Its own group, too, because it means the opposite of
        * `This project` above — that one writes `.cide/config.json`, which the repository will
        * contain and a teammate will review; this writes the profile's own file, which nobody
        * else ever sees.
        */}
      {overrides !== null && (
        <Group title="Local overrides">
          <ProjectOverrideRows
            overrides={overrides}
            roles={
              projectRoster !== null && projectRoster.kind === 'ready'
                ? projectRoster.agents.map((def) => ({
                    id: def.id,
                    label: def.label,
                    scope: def.scope,
                  }))
                : []
            }
            pools={poolNames}
            onChange={writeOverrides}
          />
          {overrideError !== null && (
            <Note title="Those overrides could not be read or written" tone="warn">
              {overrideError}
            </Note>
          )}
        </Group>
      )}

      <Note title="A role is a file">
        A role is a system prompt plus the switches a run is spawned with. It is stored as{' '}
        <code>&lt;name&gt;.md</code> — front matter and a body — and everything in the list above
        writes that file, as the row above it writes <code>.cide/config.json</code>. Nothing on
        this screen is a cide setting, so none of it rides <code>workspace.json</code> and none of
        it follows you into another project unless you make it global.
      </Note>

      {/*
        * The dialog, written here and rendered into `document.body`.
        *
        * `OverlayCard` does the portalling itself, so this stays one line and every other caller
        * of that card gets the same treatment — see its doc for the argument. The short version
        * is that `position: fixed; inset: 0` was never enough on its own: a Settings section
        * renders inside a tab panel, `layout/TabContent.module.css` makes every tab panel a
        * stacking context, and this scrim's `z-index: 60` was therefore resolved *within* that
        * panel and capped at its level — which is how the sidebar splitter, at `z-index: 1`,
        * came to paint over the modal. `SettingsTab.module.css`'s `.pane` being the scroll
        * container is the other half, and the portal answers that too.
        *
        * Rendering `null` when closed rather than hiding it: this is not a pane host, nothing
        * inside it is expensive to rebuild, and an unmount is what guarantees the focus effect
        * runs again next time.
        */}
      {/* `modelHarness` is null exactly when `draft` is — it is derived from it — but the two are
          separate values, so the third condition is what tells the compiler that. */}
      {modal !== null && draft !== null && modelHarness !== null && (
        <RoleDialog
          modal={modal}
          draft={draft}
          dirty={dirty}
          busy={busy}
          pendingLeave={pendingLeave}
          moveWarning={moveWarning}
          modelHarness={modelHarness}
          models={modelAnswers[modelHarness] ?? null}
          modelsLoading={modelsPending === modelHarness}
          saveError={saveError}
          openError={openError}
          errorsFor={errorsFor}
          onEdit={edit}
          onSave={save}
          onRequestClose={requestClose}
          onConfirmLeave={confirmLeave}
          onKeepEditing={() => setPendingLeave(null)}
        />
      )}
    </>
  )
}

/* ------------------------------------------------------------------------------- the list */

/**
 * Every definition file, one row each, with the two things a user does to one.
 *
 * # It is a list of rows, not a list of buttons
 *
 * It used to be: each row was one big `<button>` that swapped the inline form under it. Now that
 * the form is a dialog, a row carries two actions — Edit and Delete — and a `<button>` cannot
 * contain a `<button>`. So the row is a `<li>` with its own controls, which is also what makes
 * the row able to *say* things: the scope it lives in, the harness it names, and whether anything
 * would actually run it.
 *
 * # Delete confirms on the second click, and never on the first
 *
 * `agents_delete` deliberately has no confirmation in Rust — `AgentsPanel`'s Integrate and
 * `TasksPanel`'s Delete are the two precedents, and the reason is the same in all three: a modal
 * in the domain would also have to be a modal in the MCP vocabulary. So the safeguard is the
 * button's, and it is arm-then-confirm rather than `ConfirmDestructive`. That is a deliberate
 * choice against the house dialog and it turns on that dialog's own rule 1: it *names every path
 * at stake*, because its callers act on lists the user cannot check. One role file is a single
 * path the user is already pointing at, and the second click's label names it in full.
 *
 * Exactly one row can be armed, and it is identified by `scope:name` rather than by a flag: two
 * rows really can share a name across scopes, and an armed boolean would let a click on one
 * delete the other.
 */
function RoleList({
  rows,
  loading,
  selection,
  armedDelete,
  busy,
  onEdit,
  onArmDelete,
  onDisarmDelete,
  onDelete,
}: {
  rows: readonly Row[]
  loading: boolean
  selection: Selection | null
  armedDelete: Selection | null
  busy: boolean
  onEdit: (target: Selection) => void
  onArmDelete: (target: Selection) => void
  onDisarmDelete: () => void
  onDelete: (target: Selection) => void
}) {
  if (loading) return <div className={styles.listEmpty}>Reading .cide/agents/…</div>
  if (rows.length === 0) return <div className={styles.listEmpty}>No role files found.</div>

  return (
    <ul className={styles.list}>
      {rows.map((row) => {
        const file: Selection = { scope: row.scope, name: row.name }
        const active = selection !== null && selection.name === row.name && selection.scope === row.scope
        const armed =
          armedDelete !== null && armedDelete.name === row.name && armedDelete.scope === row.scope
        const state = usability(row)
        return (
          <li
            key={fileKey(row.scope, row.name)}
            className={cx(styles.row, active && styles.rowActive, row.shadowed && styles.rowShadowed)}
            aria-current={active ? 'true' : undefined}
          >
            <div className={styles.rowBody}>
              <span className={styles.rowHead}>
                <span className={styles.rowName}>{row.label}</span>
                <span className={cx(styles.badge, row.scope === 'global' && styles.badgeGlobal)}>
                  {scopeLabel(row.scope)}
                </span>
                {/* The harness the *file* names, which is not the same fact as the roster's
                    resolved one — see `Entry.harness`. "Project default" is a real and common
                    answer, not a missing value. */}
                <span className={cx(styles.badge, styles.badgeGlobal)}>{harnessLabel(row.harness)}</span>
                <span
                  className={cx(styles.status, state.kind === 'blocked' && styles.statusBlocked)}
                  data-audit="agentRowStatus"
                >
                  {usabilityLabel(state)}
                </span>
              </span>
              <span className={styles.rowFile}>
                {row.name}.md
                {/* The one fact a merged roster cannot show. A shadowed row is not a duplicate
                    and not an alternative: it is a file the user still owns, that nothing will
                    run, until the project's role of the same name is deleted. Said on the row
                    because it is only true *of this pair* and a footnote would be read as a
                    general remark. */}
                {row.shadowed && ' — shadowed: this project defines a role of the same name, and a project role wins whole-file'}
                {row.shadows && ' — wins over your global role of the same name'}
              </span>
              {/* Rust's own sentence, never a paraphrase and never a bare grey row. It is on the
                  file it is about: `rowsFor` refuses to copy it onto a shadowed row, because the
                  merged roster answered for the winner. */}
              {state.kind === 'blocked' && <span className={styles.rowBlocked}>{state.why}</span>}
            </div>

            <div className={styles.rowActions}>
              <button
                type="button"
                className={styles.act}
                disabled={busy}
                onClick={() => onEdit(file)}
              >
                Edit
              </button>
              {armed ? (
                <>
                  <button
                    type="button"
                    className={cx(styles.act, styles.actDanger)}
                    disabled={busy}
                    onClick={() => onDelete(file)}
                  >
                    Delete {row.name}.md
                  </button>
                  <button type="button" className={styles.act} onClick={onDisarmDelete}>
                    Keep it
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className={styles.act}
                  disabled={busy}
                  onClick={() => onArmDelete(file)}
                >
                  Delete
                </button>
              )}
            </div>
          </li>
        )
      })}
    </ul>
  )
}

/* ----------------------------------------------------------------------------- the dialog */

/**
 * Everything focusable inside the card, in document order, for the Tab trap below.
 *
 * `:not([disabled])` matters: Save is disabled until the draft can be saved at all, and a trap
 * that wrapped onto a disabled button would put focus nowhere and end the keyboard session.
 * `tabindex="-1"` is excluded for the same reason it exists — it means "focusable, but not a tab
 * stop".
 */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

/**
 * The role form as a modal dialog.
 *
 * # It is `OverlayCard`, not a fourth definition of what an overlay looks like
 *
 * `overlays/ModalShell.tsx` exports two things. `ModalShell` is prompt-shaped — one input, a
 * counter, a virtualised list — and is the wrong body for an eleven-field form. `OverlayCard`,
 * which was split out of it when the close confirmation arrived, is the scrim and the 620px card
 * and nothing else, and it is what `ConfirmDestructive` already sits in. Using it is what makes
 * this dialog the same object as the others: the same ground, the same radius, the same shadow,
 * the same `--scrim`, and the same `role="dialog" aria-modal="true"` with a real accessible name.
 * 620px is also exactly the width `SettingsTab.module.css` already draws this form at, so nothing
 * reflows.
 *
 * # The three things a form dialog needs that a confirmation does not
 *
 * **Dismissal cannot be free.** `ConfirmDestructive`'s rule 3 — Escape cancels, focus starts on
 * Cancel — holds there because backing out costs nothing. Backing out of *this* costs a system
 * prompt, so Escape, the scrim and Cancel all run through `onRequestClose`, and `closeRequest`
 * in `./agentsDraft` decides between closing and asking. Its doc carries the argument and the two
 * options it beat. **Escape never discards**, in any state; discarding is a click on a button
 * that says so.
 *
 * **Focus starts in the form, not on Cancel.** The user opened this to type. It lands on the
 * *name* field in both modes: it is the first field a new role needs, it is harmless on an edit,
 * and — the actual reason it is not simply "the first control" — the first control is the scope
 * `<select>`, where a stray arrow key would change which directory the file is written to. A
 * focused `<select>` turns *Down* into a file move.
 *
 * **Focus must not leave.** A Tab that walked out of the card would land in the settings nav
 * behind the scrim, where clicks do not reach and the user has no way back except the pointer.
 * The trap is the whole of `onKeyDown`'s `Tab` arm: at the last focusable, Tab wraps to the
 * first; at the first, Shift+Tab wraps to the last; anywhere in between the browser's own order
 * is left alone, so the native tab order inside the form is untouched.
 *
 * On unmount it hands focus back to whatever had it when the dialog opened — the Edit button on
 * the row, ordinarily — so a keyboard user comes back to their place in the list rather than to
 * the top of the document. A node that has since been re-rendered away simply ignores `focus()`.
 */
function RoleDialog({
  modal,
  draft,
  dirty,
  busy,
  pendingLeave,
  moveWarning,
  modelHarness,
  models,
  modelsLoading,
  saveError,
  openError,
  errorsFor,
  onEdit,
  onSave,
  onRequestClose,
  onConfirmLeave,
  onKeepEditing,
}: {
  modal: NonNullable<Modal>
  draft: Draft
  dirty: boolean
  busy: boolean
  pendingLeave: Selection | 'new' | 'close' | null
  moveWarning: string | null
  /** Carried straight through to `RoleForm`, which documents all three. */
  modelHarness: HarnessName
  models: AgentModels | null
  modelsLoading: boolean
  saveError: string | null
  /**
   * A *load* that failed, drawn here as well as beside the list.
   *
   * The same state in two places, one of which is always covered by the scrim. Switching roles
   * from the discard confirm calls the same loader the list's Edit does, so the failure can
   * arrive with the dialog open — and a sentence painted behind a modal is a sentence nobody
   * reads, which is the whole failure mode this screen's per-field errors exist to prevent.
   */
  openError: string | null
  errorsFor: (field: AgentFieldKey) => string[]
  onEdit: (field: AgentFieldKey, patch: Partial<Draft>) => void
  onSave: () => void
  onRequestClose: () => void
  onConfirmLeave: () => void
  onKeepEditing: () => void
}) {
  const card = useRef<HTMLDivElement>(null)
  const name = useRef<HTMLInputElement>(null)
  const keep = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    const returnTo = document.activeElement
    name.current?.focus()
    return () => {
      if (returnTo instanceof HTMLElement) returnTo.focus()
    }
  }, [])

  /*
   * When the discard confirm arms, focus moves to *Keep editing*.
   *
   * The same instinct that puts `ConfirmDestructive`'s focus on Cancel, and the same reason: the
   * Enter already in flight when the confirm appeared must not be the keystroke that destroys the
   * writing. It is a separate effect from the mount one so that it fires on the transition and
   * not on every render of an armed dialog.
   */
  useEffect(() => {
    if (pendingLeave !== null) keep.current?.focus()
  }, [pendingLeave])

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      // Stopped, so the window's own Escape handling does not also act on it. Not prevented:
      // there is no default here worth suppressing, and `ConfirmDestructive` sets the precedent.
      ev.stopPropagation()
      onRequestClose()
      return
    }
    if (ev.key !== 'Tab') return
    const stops = [...(card.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])]
    const first = stops[0]
    const last = stops[stops.length - 1]
    if (first === undefined || last === undefined) return
    const at = document.activeElement
    if (ev.shiftKey && at === first) {
      ev.preventDefault()
      last.focus()
    } else if (!ev.shiftKey && at === last) {
      ev.preventDefault()
      first.focus()
    }
  }

  const creating = modal.kind === 'new'
  const title = modalTitle(modal)
  const leaving =
    pendingLeave === null
      ? null
      : pendingLeave === 'close'
        ? 'Discard your unsaved edits and close this role?'
        : pendingLeave === 'new'
          ? 'Discard your unsaved edits and start a new role?'
          : `Discard your unsaved edits and open “${pendingLeave.name}”?`

  return (
    <OverlayCard label={creating ? 'New role' : `Edit ${title}`} onDismiss={onRequestClose}>
      <div className={styles.dialog} ref={card} onKeyDown={onKeyDown} data-audit="agentRoleDialog">
        <div className={styles.dialogHead}>
          <h2 className={styles.dialogTitle}>{creating ? 'New role' : title}</h2>
          <p className={styles.dialogSub}>
            {creating
              ? 'A name, a system prompt, and the switches a run of this role is spawned with. Nothing is written until you press Create role.'
              : 'Editing the definition file itself. Nothing is written until you press Save.'}
          </p>
        </div>

        {/* The body scrolls, the head and the foot do not: `OverlayCard`'s card is capped at the
            window height, and a Save button that scrolled off the bottom of a 320px system prompt
            is a Save button the user has to go looking for. */}
        <div className={styles.dialogBody}>
          <RoleForm
            draft={draft}
            nameRef={name}
            moveWarning={moveWarning}
            modelHarness={modelHarness}
            models={models}
            modelsLoading={modelsLoading}
            errorsFor={errorsFor}
            onEdit={onEdit}
          />
        </div>

        <div className={styles.dialogFoot}>
          {saveError !== null && (
            <Note title="Nothing was written" tone="warn">
              {saveError}
            </Note>
          )}
          {openError !== null && (
            <Note title="That role could not be opened" tone="warn">
              {openError}
            </Note>
          )}
          {leaving !== null ? (
            <div className={styles.footRow} data-audit="agentRoleDiscard">
              <span className={styles.leaving}>{leaving}</span>
              <button
                type="button"
                className={cx(styles.quiet, styles.dangerText)}
                onClick={onConfirmLeave}
              >
                Discard edits
              </button>
              <button ref={keep} type="button" className={styles.save} onClick={onKeepEditing}>
                Keep editing
              </button>
            </div>
          ) : (
            <div className={styles.footRow}>
              {dirty && <span className={styles.dirty}>Unsaved changes</span>}
              <button type="button" className={styles.quiet} onClick={onRequestClose}>
                Cancel
              </button>
              <button
                type="button"
                className={styles.save}
                disabled={busy || !canSave(draft)}
                onClick={onSave}
              >
                {creating ? 'Create role' : 'Save'}
              </button>
            </div>
          )}
        </div>
      </div>
    </OverlayCard>
  )
}

/* ------------------------------------------------------------------------------- the form */

/**
 * The eleven fields, and nothing else.
 *
 * Deliberately no title, no footer and no Save: it is the dialog's body and only its body. When
 * the form was inline it owned all three, and the temptation on making it a dialog was to write a
 * second form component for the modal — which is how two forms end up drawing ten fields and
 * eleven. There is one, it is this, and the chrome around it belongs to [`RoleDialog`].
 */
function RoleForm({
  draft,
  nameRef,
  moveWarning,
  modelHarness,
  models,
  modelsLoading,
  errorsFor,
  onEdit,
}: {
  draft: Draft
  /** Where the dialog puts initial focus. See its header for why it is the name and not Cancel. */
  nameRef: React.RefObject<HTMLInputElement | null>
  moveWarning: string | null
  /**
   * Which harness the Model menu is for — `effectiveHarness`, resolved by the editor.
   *
   * Resolved *above* this component rather than here, because the answer needs the project's
   * `.cide/config.json` default and this form deliberately knows nothing about the project. It
   * is also what the probe is keyed on, so a second derivation here could ask for one harness
   * and draw another.
   */
  modelHarness: HarnessName
  /** What the harness offered, or `null` for "not asked yet, or this build cannot ask". */
  models: AgentModels | null
  modelsLoading: boolean
  errorsFor: (field: AgentFieldKey) => string[]
  onEdit: (field: AgentFieldKey, patch: Partial<Draft>) => void
}) {
  return (
    <>
      <Group>
        {/* Scope first, because it decides which file everything below is written to, and
            because a user who is about to move a role should read that sentence before they
            have typed anything else. */}
        <Field
          label="Scope"
          hint={scopeHint(draft.scope)}
          errors={errorsFor('scope')}
          control={
            <Choice
              label="Scope"
              value={draft.scope}
              /*
               * A subagent's scope is fixed while the dialog is open, and the control is
               * *rendered disabled* rather than hidden so the row still says where the file is.
               *
               * There are only two moves it could offer and neither is one. Into a cide scope is
               * a **translation**, not a move — the field sets differ, so the file that arrived
               * would be one nobody wrote — and Rust refuses it on this field. Between the two
               * Claude scopes is a real file move with no reason to exist yet. Offering either
               * and then refusing it is the listed-and-silently-inert state this codebase has
               * paid for elsewhere; a control that plainly cannot be used is not.
               */
              disabled={isClaudeScope(draft.scope)}
              options={
                isClaudeScope(draft.scope)
                  ? [{ value: draft.scope, label: scopeLabel(draft.scope) }]
                  : SCOPES.filter((scope) => !isClaudeScope(scope)).map((scope) => ({
                      value: scope,
                      label: scopeLabel(scope),
                    }))
              }
              onChange={(value) => onEdit('scope', { scope: value as Scope })}
            />
          }
        />
        {moveWarning !== null && (
          <Note title="This save moves a file" tone="warn">
            {moveWarning}
          </Note>
        )}

        <Field
          label="Name"
          hint={
            isClaudeScope(draft.scope)
              ? 'Lowercase letters, digits and “-”. Claude Code keys the subagent by this value and the file it lives in may be called anything, so changing it edits the key rather than renaming the file.'
              : '1–32 characters of a–z, 0–9 and “-”. It is the file name and the word the orchestrator asks for this role by, so changing it renames the file.'
          }
          errors={errorsFor('name')}
          control={
            <input
              ref={nameRef}
              className={styles.input}
              type="text"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              aria-label="Role name"
              placeholder="code-reviewer"
              value={draft.name}
              onChange={(e) => onEdit('name', { name: e.target.value })}
            />
          }
        />

        <Field
          label="Label"
          hint="What the roster row says. Left empty, the name is title-cased — which is right for “code-reviewer” and wrong for “qa”."
          errors={errorsFor('label')}
          control={
            <input
              className={styles.input}
              type="text"
              aria-label="Role label"
              placeholder={draft.name === '' ? 'Code Reviewer' : titleCase(draft.name)}
              value={draft.label ?? ''}
              onChange={(e) => onEdit('label', { label: e.target.value })}
            />
          }
        />

        {/*
          * **The role's colour.** (M77)
          *
          * Beside the label because it answers the same question — what this role looks like in
          * a list — and because the two are read together: the colour marks the name in the
          * Agents panel, on a task's chip and above every comment the role writes.
          *
          * It edits `extras`, not a field of its own, and that is the storage decision M75 made
          * rather than an omission: `color:` is Claude Code's own key, `cide_agents::defs` reads
          * it and leaves it carried, so a save writes the file's own line back and no
          * `.claude/agents/*.md` has its keys reordered. The Meta rows below hide the key while
          * this control owns it — see `metaExtras`, which argues why that is not the same as
          * hiding it.
          *
          * `errorsFor('extras')` and no `AgentField` of its own: there is nothing here that can
          * be refused. Every reachable value is one of the eight or *Automatic*, and a value a
          * hand-edited file wrote that this build cannot use is a **note**, not a refusal — the
          * role still runs, it is simply drawn in the colour its name derives.
          */}
        <Field
          label="Colour"
          hint="What marks this role's name in the Agents panel, on a task's chip and above its comments. Automatic derives one from the role's name — stable, the same for everybody, and never the colour reserved for the orchestrator. Stored as Claude Code's own `color:` key, so a subagent that already has one arrives with it set."
          errors={errorsFor('extras')}
          control={
            <ColorChoice
              value={colorOf(draft)}
              unknown={unknownColor(draft)}
              onChange={(color) => onEdit('extras', { extras: withColor(draft.extras, color) })}
            />
          }
        />

        {/*
          * **No harness control for a subagent**, and this is the one place on the screen where
          * a field disappears rather than greying out. The others grey because the question is
          * still meaningful and the answer is fixed; here the question does not exist. A Claude
          * Code subagent runs under Claude Code by construction — `.claude/agents/` has no
          * `harness:` key, cide carries one as an unmodelled extra with a warning, and the
          * dispatch names the definition with `--agent` rather than choosing a CLI for it.
          * Drawing a disabled dropdown reading "claude" would suggest a decision was taken.
          */}
        {!isClaudeScope(draft.scope) && (
          <Field
            label="Harness"
            hint="Which CLI runs this role. Left as the project default, the role follows .cide/config.json — which is a different state from naming the same harness here, because the default can change underneath a role that never named one."
            errors={errorsFor('harness')}
            control={
              <Choice
                label="Harness"
                value={draft.harness ?? ''}
                options={[
                  { value: '', label: harnessLabel(null) },
                  ...HARNESSES.map((harness) => ({ value: harness, label: harnessLabel(harness) })),
                ]}
                onChange={(value) =>
                  onEdit('harness', { harness: value === '' ? null : (value as HarnessName) })
                }
              />
            }
          />
        )}

        <Field
          label="Description"
          hint="One line, and it is prompt text as much as UI text: the orchestrator is handed it verbatim when it asks what agents it has. Empty is legal and warned about, never refused."
          errors={errorsFor('description')}
          control={
            <input
              className={styles.input}
              type="text"
              aria-label="Role description"
              placeholder="Implements one task end to end and reports back on it."
              value={draft.description}
              onChange={(e) => onEdit('description', { description: e.target.value })}
            />
          }
        />
      </Group>

      <Group title="System prompt">
        {/* The substance of a role, and the reason this screen exists at all: it is the whole of
            what makes this a role rather than a name. It gets a real editor with real room —
            a single-line input for a document is the shape that sends people back to their
            text editor, which is what the user was objecting to. */}
        <Field
          label="Prompt"
          hint="The body of the .md file, verbatim. Markdown; every run of this role starts with it as its system prompt."
          errors={errorsFor('systemPrompt')}
          control={
            <textarea
              className={styles.prompt}
              spellCheck={false}
              aria-label="System prompt"
              placeholder="You are the developer agent for this project. …"
              rows={18}
              value={draft.systemPrompt}
              onChange={(e) => onEdit('systemPrompt', { systemPrompt: e.target.value })}
            />
          }
        />
      </Group>

      <Group title="How a run is spawned">
        <Field
          label="Model"
          hint={modelHint(modelHarness, models)}
          errors={errorsFor('model')}
          control={
            <ModelField
              harness={modelHarness}
              answer={models}
              loading={modelsLoading}
              value={draft.model ?? ''}
              onChange={(next) => onEdit('model', { model: next })}
            />
          }
        />

        {/* Suggestions plus an escape, NOT a closed list. `AgentDraft::effort` is carried
            verbatim and validated against nothing, because the set differs per harness and per
            release — a menu cide checked against would be wrong within a month. So the common
            values are one click and anything else is still typable. */}
        <Field
          label="Effort"
          hint="A harness-specific reasoning knob, passed through as written. cide checks it against nothing, so a value a newer CLI added works here today."
          errors={errorsFor('effort')}
          control={
            <Suggest
              label="Effort"
              value={draft.effort ?? ''}
              suggestions={EFFORT_SUGGESTIONS}
              placeholder="medium"
              onChange={(value) => onEdit('effort', { effort: value })}
            />
          }
        />

        <Field
          label="Permission mode"
          hint="The CLI's own vocabulary, and cide's copy of it is allowed to go stale. If a release has added a mode this list does not have, leave it unset rather than guessing."
          errors={errorsFor('permissionMode')}
          control={
            <Choice
              label="Permission mode"
              value={draft.permissionMode ?? ''}
              options={[
                { value: '', label: 'Unset (the harness decides)' },
                ...PERMISSION_MODES.map((mode) => ({ value: mode, label: mode })),
              ]}
              onChange={(value) => onEdit('permissionMode', { permissionMode: value === '' ? null : value })}
            />
          }
        />

        <Field
          label="Tools"
          hint="--allowedTools. Empty does not mean “no tools”: it means the definition does not restrict them, which is the harness's default. Names look like Read, Bash or mcp__server__tool; one per row, because a name containing a comma or a space is read as two."
          errors={errorsFor('tools')}
          control={
            <ToolRows
              tools={draft.tools}
              onChange={(tools) => onEdit('tools', { tools })}
            />
          }
        />

        <Field
          label="Max concurrent"
          hint="How many runs of this role may be live at once. Empty means the file does not say, which the loader reads as 1 — and writing 1 into every definition would freeze a default a later cide may want to change. Worktree isolation pins it to 1 anyway."
          errors={errorsFor('maxConcurrent')}
          control={
            <input
              className={cx(styles.input, styles.inputNarrow)}
              type="number"
              inputMode="numeric"
              min={1}
              max={64}
              aria-label="Max concurrent runs"
              placeholder="1"
              value={draft.maxConcurrent === null ? '' : String(draft.maxConcurrent)}
              onChange={(e) => {
                const text = e.target.value.trim()
                const parsed = Number.parseInt(text, 10)
                onEdit('maxConcurrent', {
                  maxConcurrent: text === '' || Number.isNaN(parsed) ? null : parsed,
                })
              }}
            />
          }
        />

        {/*
          * **Meta: every front-matter key cide does not model.** (M30)
          *
          * Shown on both families of scope, and the reason is not symmetry. For a Claude Code
          * subagent these are the *interesting* keys — `hooks`, `skills`, `mcpServers`,
          * `maxTurns`, `disallowedTools`, `color` — and cide is writing the file back after the
          * user changes a description, so a form that dropped them would delete somebody's hooks
          * as a side effect of fixing a typo. For a cide role the list is ordinarily empty, and
          * when it is not, the roster is already reporting the key against its own line: showing
          * it here is what turns "cide says line 6 is wrong" into something the user can act on
          * without leaving the dialog.
          *
          * Values are edited raw and written back raw. cide has no model for what is in them —
          * that is the definition of the list — so the one honest thing it can do is not touch
          * them.
          */}
        <Field
          label="Meta"
          hint={
            isClaudeScope(draft.scope)
              ? 'Keys cide does not model — hooks, skills, mcpServers, maxTurns and anything a Claude Code release adds. cide reads none of them and writes all of them back exactly as they are, so editing a description here cannot delete them. A value may span several lines; keep a block’s indentation.'
              : 'Keys cide does not read. They are kept in the file rather than dropped by a save, and the roster reports each one against its own line — so an entry here is either a typo to fix or a key written for a newer cide.'
          }
          errors={errorsFor('extras')}
          control={
            <ExtraRows
              extras={metaExtras(draft.extras)}
              onChange={(extras) =>
                onEdit('extras', { extras: withMetaExtras(draft.extras, extras) })
              }
            />
          }
        />
      </Group>

    </>
  )
}

/**
 * The role's colour, as nine swatches and a note.
 *
 * **Swatches and not a `<select>`**, which is the one place this form departs from `Choice`.
 * Every other closed vocabulary here is a list of *words* — a harness, a permission mode — and a
 * dropdown is right for those because reading the word is how you choose. A colour is chosen by
 * looking at it, and a menu of the word "cyan" makes somebody open the menu, pick, close it, and
 * check the row in another panel to find out what they picked.
 *
 * Each swatch is a real `<button type="button">`. A `<div onClick>` would be off the tab order
 * and silent to a screen reader, and this control has no text of its own at all — `aria-label`
 * is the only thing naming any of them, and `aria-pressed` the only thing saying which is on.
 *
 * The colours are drawn from the same `--agent-<hue>` tokens the panel paints with, never from a
 * hex spelled here: a swatch that stated its own colour would be this file's opinion of what
 * `cyan` looks like, and it would be one theme's opinion at that. `check:theme` gates those
 * tokens and knows nothing about this component.
 *
 * **Automatic leads**, because it is the default, it is what every role has until somebody
 * chooses, and — being the only option that *removes* the key — it is the one a reader needs to
 * find when they change their mind. It is drawn as a ringed empty circle rather than a ninth
 * colour, since the thing it means is the absence of a choice and not another choice.
 */
function ColorChoice({
  value,
  unknown,
  onChange,
}: {
  value: string | null
  unknown: string | null
  onChange: (color: string | null) => void
}) {
  return (
    <div className={styles.colorChoice} data-audit="agentColorChoice">
      <div className={styles.colorSwatches} role="group" aria-label="Role colour">
        <button
          type="button"
          className={cx(styles.colorSwatch, styles.colorAuto)}
          data-audit="agentColorAuto"
          aria-label="Automatic — derived from the role's name"
          aria-pressed={value === null}
          data-on={value === null ? 'true' : 'false'}
          title="Automatic — derived from the role's name"
          onClick={() => onChange(null)}
        />
        {AGENT_HUES.map((hue) => (
          <button
            key={hue}
            type="button"
            className={styles.colorSwatch}
            data-audit="agentColorSwatch"
            data-hue={hue}
            /* The token, never a hex — see the header. */
            style={{ background: `var(--agent-${hue})` }}
            aria-label={titleCase(hue)}
            aria-pressed={value === hue}
            data-on={value === hue ? 'true' : 'false'}
            title={titleCase(hue)}
            onClick={() => onChange(hue)}
          />
        ))}
      </div>
      {/*
        * A value the file asked for that this build cannot use. Named in full, because the
        * *point* of the sentence is that the user can see the word they typed and correct it —
        * "not a colour cide knows" without the word would send them back to the file to find out
        * which line it meant. It is a `Note` and not a refusal: the role runs, and it is drawn
        * in the colour its name derives, which is the same thing that would happen if the key
        * were absent. Nothing here rewrites it; choosing a swatch is what replaces it.
        */}
      {unknown !== null && (
        /* The hook goes on a wrapper, not on `Note`. TypeScript lets a hyphenated attribute
           through on any component — `data-*` is exempt from JSX excess-property checking — but
           `Note` does not spread its props, so it would be accepted here, dropped there, and the
           render check would look for an element that never existed. */
        <div data-audit="agentColorUnknown">
          <Note tone="warn">
            <code>{unknown}</code> is not one of the eight, so this role is drawn in the colour
            its name derives. The key is kept exactly as the file has it until you choose above.
          </Note>
        </div>
      )}
    </div>
  )
}

/**
 * The unmodelled front-matter keys, as editable key/value rows.
 *
 * Shaped on [`ToolRows`] and differing in one respect that matters: the value is a `textarea`,
 * not an `input`. A `hooks:` block arrives here as several lines with their own indentation, and
 * a single-line box would show the first line, let the user retype it, and silently discard the
 * rest the moment they touched it.
 *
 * `rows` follows the content so an ordinary one-line value does not get a text panel, and a block
 * is legible without scrolling inside a box inside a dialog.
 */
function ExtraRows({
  extras,
  onChange,
}: {
  extras: readonly Extra[]
  onChange: (next: Extra[]) => void
}) {
  const edit = (index: number, patch: Partial<Extra>) => {
    const next = extras.map((extra) => ({ ...extra }))
    next[index] = { ...next[index]!, ...patch }
    onChange(next)
  }
  return (
    <div className={styles.tools}>
      {extras.map((extra, index) => (
        // Keyed by position and key together, for `ToolRows`' reason.
        <div key={`${index}:${extra.key}`} className={styles.extraRow}>
          <input
            className={cx(styles.input, styles.inputNarrow)}
            type="text"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`Meta key ${index + 1}`}
            placeholder="maxTurns"
            value={extra.key}
            onChange={(e) => edit(index, { key: e.target.value })}
          />
          <textarea
            className={cx(styles.input, styles.extraValue)}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`Meta value ${index + 1}`}
            rows={Math.min(8, Math.max(1, extra.value.split('\n').length))}
            value={extra.value}
            onChange={(e) => edit(index, { value: e.target.value })}
          />
          <button
            type="button"
            className={styles.remove}
            aria-label={`Remove meta key ${index + 1}`}
            onClick={() => onChange(extras.filter((_, i) => i !== index))}
          >
            <Icon name="x" size={1} />
          </button>
        </div>
      ))}
      <button
        type="button"
        className={styles.add}
        onClick={() => onChange([...extras.map((e) => ({ ...e })), { key: '', value: '' }])}
      >
        + Add key
      </button>
    </div>
  )
}

/* ------------------------------------------------------------------------------- the parts */

/**
 * One labelled control with its own errors under it.
 *
 * The errors are part of the field rather than collected anywhere, which is the whole shape of
 * `AgentField`: `check-settings-agents.mjs` asserts that every variant of the Rust enum has an
 * `errorsFor('…')` call somewhere in this file, so a refusal can never arrive with nowhere to
 * land.
 */
function Field({
  label,
  hint,
  control,
  errors,
}: {
  label: string
  hint: string
  control: ReactNode
  errors: readonly string[]
}) {
  return (
    <div className={styles.field}>
      <div className={styles.fieldLabel}>{label}</div>
      {control}
      <div className={styles.fieldHint}>{hint}</div>
      {errors.map((message) => (
        // `role="alert"`, unlike `ClaudeCliSection`'s quiet `note`: this one is the answer to a
        // button the user just pressed, and it is the reason nothing was written.
        <p key={message} className={styles.error} role="alert">
          {message}
        </p>
      ))}
    </div>
  )
}

/**
 * A closed choice.
 *
 * A native `<select>` rather than the `Segmented` control the rest of Settings uses, because two
 * of the three vocabularies here do not fit on a row: six permission modes plus an unset is
 * seven segments, and a segmented control that wraps reads as a broken toolbar. The tokens are
 * the same, so it sits in the same form as the segments above it.
 */
function Choice({
  label,
  value,
  options,
  onChange,
  disabled = false,
}: {
  label: string
  value: string
  options: readonly { value: string; label: string }[]
  onChange: (next: string) => void
  /** Rendered but not answerable — the control still says what the value is. */
  disabled?: boolean
}) {
  return (
    <select
      className={styles.select}
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

/**
 * The Model box: a menu of what the harness offers, over a field that still accepts anything.
 *
 * # Why this is not a `<Choice>`
 *
 * Because `model` is deliberately **not** a closed vocabulary — `AgentDraft::model` is passed to
 * the CLI verbatim and validated against nothing, and `check-settings-agents.mjs` asserts both
 * halves of that. The list under this control is one machine's answer at one moment: for
 * opencode it is `opencode models`, which knows only about providers that are configured *now*,
 * and for Claude it is a handful of aliases that a release may add to next week. Closing the
 * field over either would start refusing a model that works.
 *
 * # Why a `<select>` and not `Suggest`'s chips
 *
 * `Suggest` is the same idea for `effort`, where the vocabulary is three words. This one is
 * thirteen ids on the machine it was written on and can be far more, several of them longer than
 * the dialog is wide. Chips would wrap into a wall.
 *
 * # The two things drawn here that are not the value
 *
 * `problem` is a fact about the **machine** — `opencode` not on `PATH`, a probe that timed out —
 * so it is a hint and never an error: nothing the user typed caused it, nothing they can type in
 * this form fixes it, and the box below stays editable throughout. And the answer is used only
 * when `answer.harness` matches the harness being asked about, because a probe in flight while
 * somebody flips the Harness dropdown would otherwise paint one CLI's ids under the other's name.
 */
function ModelField({
  harness,
  answer,
  loading,
  value,
  onChange,
}: {
  harness: HarnessName
  answer: AgentModels | null
  loading: boolean
  value: string
  onChange: (next: string) => void
}) {
  // The guard described above. `null` is a build that could not answer at all, which is drawn
  // exactly like an empty list: a plain text box, which is the field that shipped before this.
  const fresh = answer !== null && answer.harness === harness ? answer : null
  const listed = fresh?.models ?? []
  // A value the list does not contain still has to be *displayable*, or the select reads
  // "default" over a box holding a model. That includes every value typed by hand, which is the
  // case this control must not punish.
  const options = value !== '' && !listed.includes(value) ? [value, ...listed] : listed
  return (
    <div className={styles.suggest}>
      <select
        className={styles.select}
        aria-label="Model suggestions"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        <option value="">
          {loading ? 'Reading the model list…' : `The ${harnessLabel(harness)} default`}
        </option>
        {options.map((model) => (
          <option key={model} value={model}>
            {model}
          </option>
        ))}
      </select>
      <input
        className={styles.input}
        type="text"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        aria-label="Model"
        placeholder={modelPlaceholder(harness)}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  )
}

/** The sentence under the Model box, which changes with the harness and with what a probe said. */
function modelHint(harness: HarnessName, answer: AgentModels | null): string {
  const base =
    harness === 'opencode' || harness === 'mimo'
      ? `An id in ${harness}’s own spelling, provider/model. The menu is what \`${harness} models\` ` +
        'reports on this machine, so it lists the providers you have configured.'
      : harness === 'codex'
        ? 'A model slug in Codex’s own spelling. The menu is what `codex debug models` lists on ' +
          'this machine; a slug the catalog hides can still be typed.'
        : 'An alias or a full model name. The menu is the aliases this build knows; a newer one ' +
          'can still be typed.'
  const problem = answer !== null && answer.harness === harness ? answer.problem : null
  // Appended rather than replacing: the field is still usable and the sentence saying how to use
  // it is still true. Only the menu is missing.
  return problem === null ? `${base} Empty means the harness’s own default.` : `${base} ${problem}`
}

/**
 * A choice that is only a suggestion: the common values as buttons, and a text field that
 * accepts anything.
 *
 * The one control on this screen that is deliberately *not* closed. See `EFFORT_SUGGESTIONS`.
 */
function Suggest({
  label,
  value,
  suggestions,
  placeholder,
  onChange,
}: {
  label: string
  value: string
  suggestions: readonly string[]
  placeholder: string
  onChange: (next: string) => void
}) {
  return (
    <div className={styles.suggest}>
      <div className={styles.suggestRow}>
        {suggestions.map((suggestion) => (
          <button
            key={suggestion}
            type="button"
            className={cx(styles.chip, value === suggestion && styles.chipOn)}
            onClick={() => onChange(value === suggestion ? '' : suggestion)}
          >
            {suggestion}
          </button>
        ))}
      </div>
      <input
        className={styles.input}
        type="text"
        spellCheck={false}
        aria-label={label}
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  )
}

/** One tool name per row, with add and remove. The list is `--allowedTools`, in order. */
function ToolRows({ tools, onChange }: { tools: readonly string[]; onChange: (next: string[]) => void }) {
  return (
    <div className={styles.tools}>
      {tools.map((tool, index) => (
        // Keyed by content as well as position, for the reason `ClaudeCliSection`'s token list
        // spells out at length — except that these inputs are controlled, so this is belt and
        // braces rather than the fix. Two rows with the same text are the user's own doing.
        <div key={`${index}:${tool}`} className={styles.toolRow}>
          <input
            className={styles.input}
            type="text"
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            aria-label={`Tool ${index + 1}`}
            value={tool}
            onChange={(e) => {
              const next = [...tools]
              next[index] = e.target.value
              onChange(next)
            }}
          />
          <button
            type="button"
            className={styles.remove}
            aria-label={`Remove tool ${index + 1}`}
            onClick={() => onChange(tools.filter((_, i) => i !== index))}
          >
            <Icon name="x" size={1} />
          </button>
        </div>
      ))}
      <button
        type="button"
        className={styles.add}
        onClick={() => onChange([...tools, ''])}
      >
        + Add tool
      </button>
    </div>
  )
}


/**
 * The local override table for one project. (M45)
 *
 * # Why this is not a settings row, and says so
 *
 * Everything else in this group writes `.cide/config.json`, which the repository will contain and
 * a teammate will review. This writes `agent-overrides.json` in the profile's own config
 * directory, which nobody else ever sees — and that difference is the entire reason the layer
 * exists. `cide_ipc::overrides`' header carries the argument: a `pool:` key in a committed role
 * file would make a teammate's clone name a pool they do not have, on every dispatch, for ever.
 *
 * So the heading and the note say "not committed" in as many words. A row that looked like the
 * one above it and meant the opposite would be the lie `OrchestrationConfig`'s doc warns about,
 * arriving from the other direction.
 */
function ProjectOverrideRows({
  overrides,
  roles,
  pools,
  onChange,
}: {
  overrides: ProjectOverrides
  roles: readonly { id: string; label: string; scope: string }[]
  /** Pool names. A stable reference: derived once in a memo, not rebuilt per store read. */
  pools: readonly string[]
  onChange: (next: ProjectOverrides) => void
}) {
  const poolList = pools
  const overridden = Object.keys(overrides.roles)
  // The project-wide row exists only once somebody has asked for it. An override is a
  // *redirection*, and a redirection nobody made should not be drawn as an empty form the user
  // has to read past — the screen's default state is "this project runs what it committed".
  const hasAll = !isEmptyOverride(overrides.all)
  // A Claude Code role cannot be redirected: `cide_agents::defs` forces `Harness::Claude` for a
  // `.claude/agents/` file because "a Claude Code subagent runs under Claude Code; there is no
  // second answer". Offering it here would promise something the fork reverses in silence.
  const eligible = roles.filter(
    (role) =>
      role.scope !== 'claudeProject' &&
      role.scope !== 'claudeGlobal' &&
      !overridden.includes(role.id),
  )

  const setAll = (next: AgentOverride) => onChange({ ...overrides, all: next })
  const setRole = (id: string, next: AgentOverride) =>
    onChange({ ...overrides, roles: { ...overrides.roles, [id]: next } })
  const dropRole = (id: string) => {
    const rest = { ...overrides.roles }
    delete rest[id]
    onChange({ ...overrides, roles: rest })
  }

  return (
    <>
      <Note title="Not committed">
        These redirect roles on <strong>this machine only</strong>. They are stored in your
        profile&rsquo;s <code>agent-overrides.json</code>, never in <code>.cide/</code>, so a
        teammate cloning this repository gets each role&rsquo;s committed harness and never sees a
        pool they do not have. Pools themselves are defined on the Models screen in Settings.
        {poolList.length === 0 && ' You have not defined any pool yet.'}
      </Note>

      {hasAll && (
        <OverrideFields
          title="Every role in this project"
          value={overrides.all}
          pools={poolList}
          // Removing the project-wide row is clearing it, which is the same gesture as clearing
          // every field — so it is one button rather than a rule the user has to discover.
          onRemove={() => setAll({})}
          onChange={setAll}
        />
      )}

      {overridden.map((id) => (
        <OverrideFields
          key={id}
          title={id}
          value={overrides.roles[id] ?? {}}
          pools={poolList}
          onChange={(next) => setRole(id, next)}
          onRemove={() => dropRole(id)}
          /* A role's own row REPLACES the project default rather than merging with it, which is
             `ProjectOverrides::for_role`'s rule — a half-merge would hand a claude role a pool its
             harness cannot use. Said here because it is not guessable from the layout. */
          note={hasAll ? 'Replaces the project-wide row above for this role, rather than adding to it.' : undefined}
        />
      ))}

      {!hasAll && overridden.length === 0 && (
        <p className={styles.overrideNote}>
          Nothing is overridden here, so every role runs what its file says.
        </p>
      )}

      <div className={styles.overrideActions}>
        {!hasAll && (
          <ActionButton
            label="Override all roles"
            // Seeded with the harness rather than empty, so the row it opens already says
            // something: an override with every field unset is indistinguishable from no
            // override, and `hasAll` would read it as absent and hide the form again.
            onClick={() => setAll({ harness: 'opencode' })}
          />
        )}
        {eligible.length > 0 && (
          <Select
            label="Override one role"
            value=""
            options={[
              { value: '', label: 'Override one role…' },
              ...eligible.map((role) => ({ value: role.id, label: role.label || role.id })),
            ]}
            onChange={(id) => {
              if (id !== '') setRole(id, { harness: 'opencode' })
            }}
          />
        )}
      </div>
    </>
  )
}

/**
 * Does this override say nothing at all?
 *
 * The screen's "is there a project-wide row" test. It has to treat a *missing* field and a field
 * that came back `null` alike: `AgentOverride`'s fields are `skip_serializing_if`, so an unset one
 * is absent — but a file hand-edited, or written by a build before that annotation, can still
 * carry nulls, and reading one as "set" would draw a form the user cannot clear.
 */
function isEmptyOverride(value: AgentOverride): boolean {
  return (
    value.harness == null &&
    value.pool == null &&
    value.model == null &&
    value.effort == null &&
    value.maxConcurrent == null &&
    value.permissionMode == null
  )
}

/**
 * Set one field of an override, or clear it by **omitting the key**.
 *
 * `AgentOverride`'s fields are `#[ts(optional)]`, so they arrive as `field?: T` — and under
 * `exactOptionalPropertyTypes` "absent" and "present but `undefined`" are different types, only
 * the first of which serde reads back as `None`. Assigning `undefined` would compile in a laxer
 * config and send a key whose value the backend cannot parse.
 */
function withField<K extends keyof AgentOverride>(
  value: AgentOverride,
  key: K,
  next: NonNullable<AgentOverride[K]> | undefined,
): AgentOverride {
  const out: AgentOverride = { ...value }
  if (next === undefined) {
    delete out[key]
  } else {
    out[key] = next
  }
  return out
}

/** The six fields of one override, drawn identically for the project row and a role's own. */
function OverrideFields({
  title,
  value,
  pools,
  onChange,
  onRemove,
  note,
}: {
  title: string
  value: AgentOverride
  pools: readonly string[]
  onChange: (next: AgentOverride) => void
  onRemove?: (() => void) | undefined
  note?: string | undefined
}) {
  // `pool` and `model` are mutually exclusive — naming both would be asking for one model and a
  // list of models at once — so one control picks the mode and setting either clears the other.
  // `!= null`, not `!== undefined`: an unset field is absent on the wire, but a file written
  // before `skip_serializing_if` — or hand-edited — can carry a null, and reading one as "set"
  // is a control that cannot be returned to "leave as committed".
  const mode = value.pool != null ? 'pool' : value.model != null ? 'model' : 'none'
  return (
    <div className={styles.overrideCard}>
      <div className={styles.overrideHead}>
        <span className={styles.overrideTitle}>{title}</span>
        {onRemove !== undefined && (
          <button
            type="button"
            className={styles.overrideRemove}
            aria-label={`Stop overriding ${title}`}
            onClick={onRemove}
          >
            <Icon name="x" size={1} />
          </button>
        )}
      </div>
      {note !== undefined && <p className={styles.overrideNote}>{note}</p>}

      <SettingRow
        label="Harness"
        hint="Which CLI runs this here. “Leave as committed” keeps whatever the role file says."
        control={
          <Select
            label="Harness"
            value={value.harness ?? ''}
            options={[
              { value: '', label: 'Leave as committed' },
              ...HARNESSES.map((h) => ({ value: h, label: harnessLabel(h) })),
            ]}
            onChange={(next) =>
              onChange(withField(value, 'harness', next === '' ? undefined : (next as HarnessName)))
            }
          />
        }
      />

      <SettingRow
        label="Model choice"
        hint={
          'A pool falls down its list when a provider rate-limits, cannot be reached, or refuses ' +
          'the credential. A single model does not. Only opencode runs use either.'
        }
        control={
          <Select
            label="Model choice"
            value={mode}
            options={[
              { value: 'none', label: 'Leave as committed' },
              { value: 'pool', label: 'A pool' },
              { value: 'model', label: 'One model' },
            ]}
            onChange={(next) =>
              onChange(
                withField(
                  withField(value, 'pool', next === 'pool' ? (value.pool ?? '') : undefined),
                  'model',
                  next === 'model' ? (value.model ?? '') : undefined,
                ),
              )
            }
          />
        }
      />

      {mode === 'pool' && (
        <SettingRow
          label="Pool"
          hint={
            pools.length === 0
              ? 'No pool is defined yet — add one on the Models screen in Settings.'
              : 'A run starts on the first entry and only moves down when one refuses.'
          }
          control={
            <Select
              label="Pool"
              value={value.pool ?? ''}
              options={[
                { value: '', label: 'Pick a pool…' },
                ...pools.map((name) => ({ value: name, label: name })),
              ]}
              onChange={(pool: string) => onChange(withField(value, 'pool', pool))}
            />
          }
        />
      )}

      {mode === 'model' && (
        <TextField
          label="Model"
          hint="An id in opencode’s own spelling, provider/model."
          value={value.model ?? ''}
          placeholder="openrouter/deepseek/deepseek-chat"
          onCommit={(model: string) => onChange(withField(value, 'model', model))}
        />
      )}

      <TextField
        label="Effort"
        hint="The variant a run asks for. A pool entry that names its own wins over this."
        value={value.effort ?? ''}
        placeholder="leave as committed"
        onCommit={(effort: string) =>
          onChange(withField(value, 'effort', effort === '' ? undefined : effort))
        }
      />

      <SettingRow
        label="Permission mode"
        hint={
          'How runs answer permission prompts here. Beats the role file and the project default ' +
          '(auto). auto lets Claude’s classifier approve on your behalf; bypassPermissions lets a ' +
          'run do anything without asking. codex and qwen map each word onto their own modes ' +
          '(codex has nobody to ask, so it refuses manual), and opencode ignores it.'
        }
        control={
          <Select
            label="Permission mode"
            value={value.permissionMode ?? ''}
            options={[
              { value: '', label: 'Leave as committed' },
              ...PERMISSION_MODES.map((mode) => ({ value: mode, label: mode })),
            ]}
            onChange={(next) =>
              onChange(withField(value, 'permissionMode', next === '' ? undefined : next))
            }
          />
        }
      />

      <SettingRow
        label="Concurrent runs of this role"
        hint="0 leaves the role's own max-concurrent alone. Useful for more parallelism against a local model that costs nothing."
        control={
          <NumberField
            label="Concurrent runs of this role"
            value={value.maxConcurrent ?? 0}
            min={0}
            max={64}
            onChange={(next) =>
              onChange(withField(value, 'maxConcurrent', next === 0 ? undefined : next))
            }
          />
        }
      />
    </div>
  )
}
