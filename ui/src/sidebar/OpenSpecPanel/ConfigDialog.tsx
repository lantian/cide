/**
 * This project's `openspec/config.yaml`, as a modal off the OpenSpec panel. (M28)
 *
 * The host. `ConfigForm.tsx` holds the two pure views and `configModel.ts` holds every rule
 * either of them turns on; this file owns the state, the calls and the decision about which of
 * the two screens the project is on.
 *
 * # Why here, and not under Settings — where it shipped
 *
 * It was `SettingsSection::OpenSpec`, on the argument that `AgentsSection` next door already
 * answers "why is a committed file edited under Settings": because *"I don't want to configure a
 * yaml, I want a UI for that"* is the complaint, and Settings is where a person looks for a UI to
 * configure something. That argument was about *discoverability* and it ignored a plainer fact,
 * which is the one a user reported: **cide's Settings is global and this file is not.** Nine of
 * the eleven sections write `Settings`, which follows a person into every repository they open;
 * this one writes one YAML file in one project, and putting it in that list says it is a
 * preference. It is a fact about the repository, and it is committed.
 *
 * The gear that opens this sits on the OpenSpec panel's own header, which is the surface that is
 * already scoped to exactly one project — so the scope is stated by *where the control is* rather
 * than by a paragraph under a heading that contradicts it.
 *
 * The counter-argument the old doc made — that the panel is a **board** and a modal editing
 * something that is not one of its rows would be "a settings screen wearing a board's clothes" —
 * is answered by putting the control in the *header* rather than in the tree. A panel header is
 * where a panel's own affordances live; nothing about a change row offers this.
 *
 * # Three things this screen does that most settings screens do not
 *
 * **It writes a file, not a setting.** `useSettings.patch` is fire-and-forget because a
 * `cide://workspace-changed` snapshot follows every settings write and a switch whose write
 * failed flicks back on its own. There is no such snapshot for `openspec/config.yaml`, so nothing
 * in the window would ever contradict a Save that did not happen. Every call here is awaited and
 * every failure is drawn, through `errorText` and never `String(e)`.
 *
 * **It is two screens, and the CLI decides which.** A project with no `openspec/` gets the
 * wizard; everything else gets the form. The board is what says which — not `SpecConfig.exists` —
 * because a project can perfectly well have an `openspec/` that predates `config.yaml`, and
 * offering to run `openspec init` in a directory that is already set up is a gesture nobody wants
 * to find out the meaning of.
 *
 * **A failed board does not take the form away.** `spec_board` spawns the CLI, and the CLI can be
 * missing. The *config* file is read and written by cide's own line-oriented scanner with no
 * subprocess at all, so a machine with no `openspec` on PATH is a machine where this screen still
 * works — the schema list degrades to one row and says so. Only the `absent` arm, which is a
 * positive statement that the directory is not there, switches to the wizard; `unusable` and
 * `unknown` fall through to the form.
 */
import { useCallback, useEffect, useState } from 'react'
import type { ProjectId, SpecBoard, SpecConfig, SpecSchema } from '@/ipc/client'
import { file, spec, specConfig } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { OverlayCard } from '@/overlays/ModalShell'
import { Icon, asIcon } from '@/icons/Icon'
import { ConfigForm, ConfigWizard, type SaveStatus } from './ConfigForm'
import { reloadConsoleAfterSetUp } from './consoleReload'
import { draftFrom, editsFor, isDirty, type ConfigDraft } from './configModel'
import styles from './ConfigForm.module.css'

/**
 * The modal, over the whole window.
 *
 * `OverlayCard` and not a panel-sized popover: the form is a long one — a schema picker, a
 * multi-line project context, per-artifact rules and per-operation guidance — and the sidebar is
 * a column somebody has already dragged to the width they want their *tree* at. It also gets the
 * scrim, `role="dialog"`, `aria-modal` and dismissal for free, none of which is worth a second
 * hand-rolled copy of.
 *
 * `project` comes from the panel rather than from the store: the panel is already scoped to one
 * project and passing it down is what makes that scope the same one, by construction, rather than
 * by two components agreeing about which project is active.
 */
export function ConfigDialog({
  project,
  onClose,
}: {
  project: ProjectId
  onClose: () => void
}) {
  /*
   * Escape on the card rather than on `document`: a window listener would also answer for the
   * terminal underneath, and for any other overlay that happens to be open. `ChangelistDialog`
   * makes the same call for the same reason.
   *
   * `autoFocus` on the close button is what makes it work before anything else is touched — the
   * card is portalled to `document.body`, so without focus inside it the keydown never reaches
   * this handler.
   */
  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onClose()
    }
  }

  return (
    <OverlayCard label="OpenSpec configuration" onDismiss={onClose}>
      <div className={styles.dialog} data-audit="specConfigDialog" onKeyDown={onKeyDown}>
        <div className={styles.dialogHead}>
          <span className={styles.dialogTitle}>OpenSpec — this project</span>
          <button
            type="button"
            className={styles.dialogClose}
            data-audit="specConfigClose"
            title="Close (Escape)"
            autoFocus
            onClick={onClose}
          >
            <Icon name={asIcon('x')} size={1} label="Close" />
          </button>
        </div>
        {/*
          * Keyed on the project, so switching projects rebuilds the whole screen rather than
          * leaving one project's unsaved draft sitting over another project's file. A draft
          * carried across would be Saved into the wrong repository.
          */}
        <OpenSpecEditor key={project} project={project} />
      </div>
    </OverlayCard>
  )
}

function OpenSpecEditor({ project }: { project: ProjectId }) {
  const [board, setBoard] = useState<SpecBoard | null>(null)
  const [config, setConfig] = useState<SpecConfig | null>(null)
  const [schemas, setSchemas] = useState<SpecSchema[]>([])
  const [draft, setDraft] = useState<ConfigDraft | null>(null)
  const [status, setStatus] = useState<SaveStatus>('idle')
  const [error, setError] = useState<string | null>(null)
  const [wizardContext, setWizardContext] = useState('')
  const [busy, setBusy] = useState(false)

  /**
   * Load everything the screen needs, in one pass.
   *
   * `Promise.all` and not three awaits in a row: `spec_board` and `spec_schemas` each spawn a
   * Node process, and running them one after another would make opening this section cost two
   * process launches back to back for no reason. `spec_config_get` spawns nothing.
   *
   * `cancelled` is the ordinary guard — a project switch unmounts this while the calls are in
   * flight, and a `setState` after that is a React warning at best and, when the two projects'
   * answers race, a form showing one project's file under another project's key.
   */
  const load = useCallback(async () => {
    const [nextBoard, nextConfig, nextSchemas] = await Promise.all([
      spec.board(project),
      specConfig.get(project),
      specConfig.schemas(project),
    ])
    return { nextBoard, nextConfig, nextSchemas }
  }, [project])

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const { nextBoard, nextConfig, nextSchemas } = await load()
        if (cancelled) return
        setBoard(nextBoard)
        setConfig(nextConfig)
        setSchemas(nextSchemas)
        setDraft(draftFrom(nextConfig))
      } catch (e) {
        if (cancelled) return
        // A read that failed must not leave an empty form on screen: an empty form invites a
        // Save that writes the emptiness back over a file nobody could read.
        setError(errorText(e))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [load])

  const onSave = useCallback(async () => {
    if (draft === null || config === null) return
    const edits = editsFor(draft, config, schemas)
    setStatus('saving')
    setError(null)
    try {
      // One call for the whole form. `config::apply` writes once, or — when every value already
      // reads that way — not at all, and does not even move the mtime.
      const next = await specConfig.set(project, edits)
      setConfig(next)
      setDraft(draftFrom(next))
      // "Nothing needed writing" is a success and not a Save, and the two want different words:
      // saying "Saved" for a write that did not happen claims a commit `git status` will not show.
      setStatus(edits.length === 0 ? 'unchanged' : 'saved')
    } catch (e) {
      setStatus('idle')
      setError(errorText(e))
    }
  }, [project, draft, config, schemas])

  const onRevert = useCallback(() => {
    if (config === null) return
    setDraft(draftFrom(config))
    setStatus('idle')
    setError(null)
  }, [config])

  const onSetUp = useCallback(async () => {
    setBusy(true)
    setError(null)
    try {
      // The context rides along with `init` rather than following it as a second call: Rust
      // writes it through the same comment-preserving writer once the file exists, so there is no
      // window in which the project is set up and the answer the user just typed is not in it.
      const nextBoard = await specConfig.setUp(project, wizardContext)
      const [nextConfig, nextSchemas] = await Promise.all([
        specConfig.get(project),
        specConfig.schemas(project),
      ])
      setBoard(nextBoard)
      setConfig(nextConfig)
      setSchemas(nextSchemas)
      setDraft(draftFrom(nextConfig))
      /*
       * The same follow-through as the absent screen's *Set up OpenSpec*, because this is the
       * same gesture behind a different button: the conversation already open in this project
       * read its skills at launch and cannot know the ones init just wrote. Not awaited into
       * `busy` — the respawn takes seconds the form does not need to spend inert, and the
       * helper never rejects: it reports each of its outcomes as a notice of its own.
       */
      void reloadConsoleAfterSetUp(project)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [project, wizardContext])

  const onOpenFile = useCallback(() => {
    if (config === null) return
    // Deliberately not awaited into an error banner: this is a navigation, and `tab_open_file`
    // mints a tab for a path whether or not it exists. A rejection here reaches `Failures`,
    // which is where a gesture's refusal belongs.
    void file.open(project, config.path)
  }, [project, config])

  if (config === null || draft === null) {
    /*
     * Two states, and the empty one was drawn as nothing.
     *
     * `spec_config_get` is a file read with no subprocess behind it — but it is one of *three*
     * calls this screen makes, and the other two (`spec_board`, `spec_schemas`) each spawn the
     * `openspec` CLI, which is a node process. Opening the dialog therefore costs the better part
     * of a second, and for all of it the card was a titled frame with nothing in it: the same
     * failure the task card had, in the same feature, for the same reason — an in-flight read
     * rendered as an absence.
     *
     * A read that failed is the other state and must never leave an *empty form* on screen: an
     * empty form invites a Save that writes the emptiness back over a file nobody could read.
     */
    return error === null ? (
      <p className={styles.reading} data-audit="specConfigReading">
        Reading this project’s OpenSpec configuration…
      </p>
    ) : (
      <div className={styles.readFailed} data-audit="specConfigFailed">
        <p className={styles.readFailedHead}>
          This project’s OpenSpec configuration could not be read
        </p>
        <p className={styles.readFailedLine}>{error}</p>
      </div>
    )
  }

  if (board !== null && board.kind === 'absent') {
    return (
      <ConfigWizard
        path={board.path}
        hint={board.hint}
        context={wizardContext}
        onContext={setWizardContext}
        busy={busy}
        error={error}
        onSetUp={() => void onSetUp()}
      />
    )
  }

  return (
    <ConfigForm
      config={config}
      schemas={schemas}
      draft={draft}
      onDraft={(next) => {
        setDraft(next)
        // Any keystroke retires the last verdict: a screen still saying "Saved" over a form that
        // has since been edited is the one thing worse than saying nothing.
        setStatus('idle')
      }}
      dirty={isDirty(draft, config, schemas)}
      status={status}
      error={error}
      cliNote={board !== null && board.kind === 'unusable' ? board.reason : null}
      onSave={() => void onSave()}
      onRevert={onRevert}
      onOpenFile={onOpenFile}
    />
  )
}
