/**
 * Content mode C: the Settings tab.
 *
 * 190px nav on the left with the mock's seven sections in the mock's order, the active row on
 * `--chrome-hi`; the section on the right at 34px/44px padding under a 20px/600 title and a
 * dim description.
 *
 * The selected section is local state seeded from the tab, and is *also* pushed back to Rust
 * so it survives a relaunch. Local first because a nav click that waits an IPC round trip
 * before moving reads as a stuck button; pushed back because `TabKind::Settings { section }`
 * is persisted with the rest of the workspace and a tab that always reopens on Appearance
 * would be quietly discarding state it claims to keep.
 *
 * Everything below the nav is presentational — `sections.tsx` takes settings and callbacks —
 * so this component is the only place in the screen that touches the store.
 */
import { useCallback, useEffect, useState } from 'react'
import {
  agentDefs,
  claudeTasks,
  openLogDir,
  settings as settingsApi,
  type AgentModels,
  type ClaudeCliSupport,
  type ProjectId,
  type SettingsSection,
} from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { SECTIONS, renderSection } from './sections'
import { useSettings, useSettingsActions } from './useSettings'
import styles from './SettingsTab.module.css'

/**
 * The section the projectless frame was last left on, for the length of this session. (M74)
 *
 * Module scope because the frame unmounts when it is closed and there is nowhere durable to put
 * this: `TabKind::Settings { section }` is the bookmark with a project, and with none there is
 * no tab to carry one. A `useState` in the frame would forget on every close, so reopening
 * would always land on Appearance — the exact behaviour the stored section exists to avoid.
 *
 * Deliberately *not* promoted to `Workspace`: a relaunch with no project open is a fresh start,
 * and a persisted section would be a new field, a new default and a new way for a hand-edited
 * document to be wrong, bought for a nav click.
 */
let frameSection: SettingsSection = 'appearance'

/** Where the projectless Settings frame should open. See [`frameSection`]. */
export function lastFrameSection(): SettingsSection {
  return frameSection
}

export interface SettingsTabProps {
  /**
   * The project owning this tab. Settings are global; the tab is not.
   *
   * `null` is the **projectless frame** (M74): with nothing open there is no tab to put this
   * screen in, so `App.tsx` draws it in the work area instead and the screen is otherwise
   * identical. Three things follow from it and nothing else does — the section is remembered in
   * [`frameSection`] rather than on a tab, and the two model probes run in the user's own
   * directory rather than a project's, which is what a global setting is about anyway.
   */
  project: ProjectId | null
  /** The section the tab was last left on. */
  section: SettingsSection
}

export function SettingsTab({ project, section }: SettingsTabProps) {
  const [active, setActive] = useState<SettingsSection>(section)
  const settings = useSettings()
  const version = useWorkspace((s) => s.boot?.capabilities.version ?? null)
  const claudeVersion = useWorkspace((s) => s.boot?.capabilities.claudeVersion ?? null)
  const { patch, setTheme, setWindowMode } = useSettingsActions()

  /**
   * The CLI verdict — can the configured binary be run, and is its version one this build's
   * protocol was checked against.
   *
   * # Re-fetched when the configured binary changes, and that is the whole reason for the dep
   *
   * The probe used to be latched in Rust for the life of the process, which was free and
   * correct while `claude` was a bare name nobody could change. It is a *setting* now: a user
   * who fixes a typo in the Binary field and reads back the verdict for the binary that
   * answered ten minutes ago has been told their correction did not work. So the Rust side is
   * unlatched and per-binary, and this effect re-runs on the value it is about.
   *
   * The cost is one `claude --version` — a Node boot, hundreds of milliseconds, on Rust's
   * blocking pool — per *distinct* binary, not per keystroke: the field commits on blur, so
   * `settings.claude.cli.binary` changes once per edit rather than once per character.
   *
   * # …and when the injections change, which is a second reason and a newer one
   *
   * `ClaudeCliSupport` stopped being a fact about a binary the moment the injection switches
   * existed. `argReasons` is now keyed by the flag cide will *actually* write — a refusal lifts
   * when an injection is switched off and moves when it is renamed — and `injectReasons` exists
   * only for the configuration that produced it. Keyed on the binary alone this effect never
   * re-ran for either, so a user who typed `sid` into a flag field got the input struck through
   * with **no sentence under it**, and a `--sid` of their own struck through with the sentence
   * for a flag nobody passes. Both are the exact state `CliReason` was added to end, and the
   * only cure was editing the binary or closing the tab.
   *
   * Serialised rather than depended on by identity: `settings` is a fresh object on every
   * workspace snapshot, so the reference changes constantly and the string does not. It changes
   * when a toggle or a spelling does, and both commit on blur or on click — the same order of
   * frequency the binary field already pays a probe for.
   *
   * `cliSupport` degrades to a verdict-free value rather than throwing, so a build without the
   * command still draws.
   */
  // `null` until bootstrap resolves. Read as `undefined` rather than defaulted to `'claude'`,
  // so the first probe waits for the real value instead of firing once for the default and
  // again for whatever the user actually configured.
  const configuredBinary = settings?.claude.cli.binary
  const configuredInjections = JSON.stringify(settings?.claude.cli.inject ?? null)
  const [cliSupport, setCliSupport] = useState<ClaudeCliSupport | null>(null)
  useEffect(() => {
    if (configuredBinary === undefined) return
    let live = true
    void claudeTasks.cliSupport().then((support) => {
      if (live) setCliSupport(support)
    })
    return () => {
      live = false
    }
  }, [configuredBinary, configuredInjections])

  /**
   * What `opencode models` reports, with cide's provider document injected. (M45)
   *
   * Refetched whenever the provider list changes — a key typed or an endpoint corrected is
   * precisely when somebody wants to know whether it worked — and keyed on the *providers* only,
   * so editing a pool (which changes nothing opencode can see) does not spawn a process.
   * `recheck` is the same fetch on demand.
   */
  const configuredProviders = JSON.stringify(settings?.llm.providers ?? null)
  const [opencodeModels, setOpencodeModels] = useState<AgentModels | null>(null)
  const [modelsNonce, setModelsNonce] = useState(0)
  useEffect(() => {
    if (settings === undefined) return
    let live = true
    void agentDefs
      .models(project, 'opencode')
      .then((answer) => {
        if (live) setOpencodeModels(answer)
      })
      // A build with no such handler, or a project that has gone away. The screen draws the
      // un-probed state, which is the same one it starts in.
      .catch(() => {
        if (live) setOpencodeModels(null)
      })
    return () => {
      live = false
    }
  }, [project, configuredProviders, modelsNonce, settings === undefined])
  const recheckModels = useCallback(() => setModelsNonce((n) => n + 1), [])
  /**
   * One real turn against one model, for the Models screen's Test buttons.
   *
   * Not memoised on anything but `project`: it takes the model as an argument and reads nothing
   * else, so a provider edit must not give the buttons a new identity mid-test.
   */
  const testModel = useCallback(
    (model: string) => agentDefs.testModel(project, model),
    [project],
  )

  /**
   * The log directory, resolved by opening it.
   *
   * Not fetched on mount: the path is only interesting once the user has asked for it, and
   * the command's whole job is the side effect. It is kept afterwards so the machine where no
   * file manager answered still ends up showing a path that can be copied — see
   * `PathReadout`.
   */
  const [logDir, setLogDir] = useState<string | null>(null)
  const revealLogDir = useCallback(() => {
    void openLogDir()
      .then(setLogDir)
      // Rejects only when the platform has no log directory or it could not be created,
      // neither of which the user can act on beyond reading the log — which is the thing they
      // cannot reach. The Rust side has already logged it.
      .catch(() => setLogDir(null))
  }, [])

  const select = useCallback(
    (next: SettingsSection) => {
      setActive(next)
      if (project === null) {
        // The projectless frame: no tab to record it on, so it is remembered for the session.
        frameSection = next
        return
      }
      // Fire and forget: the section is a bookmark, and failing to record it costs the user
      // one nav click after a relaunch. Blocking the click on it would cost every click.
      void settingsApi.openTab(project, next).catch(() => {})
    },
    [project],
  )

  // `SECTIONS` is non-empty and `active` always names one of its entries, but the compiler
  // has `noUncheckedIndexedAccess` and cannot know either — and a hand-written fallback here
  // is cheaper than the assertion that would silence it.
  const meta = SECTIONS.find((s) => s.id === active) ?? {
    title: 'Settings',
    description: '',
  }

  return (
    <div className={styles.tab}>
      <nav
        className={styles.nav}
        role="tablist"
        aria-orientation="vertical"
        aria-label="Settings sections"
      >
        {SECTIONS.map((entry) => {
          const selected = entry.id === active
          return (
            <button
              key={entry.id}
              type="button"
              role="tab"
              aria-selected={selected}
              className={selected ? `${styles.navItem} ${styles.navItemActive}` : styles.navItem}
              onClick={() => select(entry.id)}
            >
              {entry.title}
            </button>
          )
        })}
      </nav>

      <div className={styles.pane} role="tabpanel" aria-label={meta.title}>
        <h2 className={styles.title}>{meta.title}</h2>
        <p className={styles.description}>{meta.description}</p>
        <div className={styles.body}>
          {settings === null ? (
            // Bootstrap has not resolved. Rendering the form against defaults would show
            // values that are not the user's and then swap them under the pointer.
            <div className={styles.loading}>Loading settings…</div>
          ) : (
            renderSection(active, {
              settings,
              patch,
              setTheme,
              setWindowMode,
              version,
              claudeVersion,
              cliSupport,
              openLogDir: revealLogDir,
              logDir,
              opencodeModels,
              recheckModels,
              testModel,
            })
          )}
        </div>
      </div>
    </div>
  )
}
