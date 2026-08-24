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
import { agentDefs, agents as agentsApi, type ProjectId } from '@/ipc/client'
import { OverlayCard } from '@/overlays/ModalShell'
import { useAgents } from '@/sidebar/agentsStore'
import { useActiveProject } from '@/store/workspace'
import { ActionButton, Group, Note, PathReadout } from './controls'
import {
  EFFORT_SUGGESTIONS,
  HARNESSES,
  PERMISSION_MODES,
  SCOPES,
  blankDraft,
  canSave,
  closeRequest,
  fileKey,
  fromWire,
  harnessLabel,
  isDirty,
  localProblems,
  modalFor,
  modalTitle,
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
  usability,
  usabilityLabel,
  type AgentFieldKey,
  type Draft,
  type Entry,
  type Modal,
  type Problem,
  type Row,
  type Scope,
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
        body: `Nothing in .cide/agents/ or in your global roles. New role writes the first one; ${roster.configPath} is where the project's own switches live.`,
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

  useEffect(refresh, [refresh])

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

      <Note title="A role is a file">
        A role is a system prompt plus the switches a run is spawned with. It is stored as{' '}
        <code>&lt;name&gt;.md</code> — front matter and a body — and everything on this screen
        writes that file; nothing here is a cide setting, so none of it rides{' '}
        <code>workspace.json</code> and none of it follows you into another project unless you
        make it global.
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
      {modal !== null && draft !== null && (
        <RoleDialog
          modal={modal}
          draft={draft}
          dirty={dirty}
          busy={busy}
          pendingLeave={pendingLeave}
          moveWarning={moveWarning}
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
  errorsFor,
  onEdit,
}: {
  draft: Draft
  /** Where the dialog puts initial focus. See its header for why it is the name and not Cancel. */
  nameRef: React.RefObject<HTMLInputElement | null>
  moveWarning: string | null
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
              options={SCOPES.map((scope) => ({ value: scope, label: scopeLabel(scope) }))}
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
          hint="1–32 characters of a–z, 0–9 and “-”. It is the file name and the word the orchestrator asks for this role by, so changing it renames the file."
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
                onEdit('harness', { harness: value === '' ? null : (value as 'claude' | 'opencode') })
              }
            />
          }
        />

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
          hint="An alias or a full model name. Empty means the harness's own default, which is the right answer for a role that does not care."
          errors={errorsFor('model')}
          control={
            <input
              className={styles.input}
              type="text"
              spellCheck={false}
              aria-label="Model"
              placeholder="sonnet"
              value={draft.model ?? ''}
              onChange={(e) => onEdit('model', { model: e.target.value })}
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
      </Group>

    </>
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
}: {
  label: string
  value: string
  options: readonly { value: string; label: string }[]
  onChange: (next: string) => void
}) {
  return (
    <select
      className={styles.select}
      aria-label={label}
      value={value}
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
