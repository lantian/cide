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
  claudeTasks,
  openLogDir,
  settings as settingsApi,
  type ClaudeCliSupport,
  type ProjectId,
  type SettingsSection,
} from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { SECTIONS, renderSection } from './sections'
import { useSettings, useSettingsActions } from './useSettings'
import styles from './SettingsTab.module.css'

export interface SettingsTabProps {
  /** The project owning this tab. Settings are global; the tab is not. */
  project: ProjectId
  /** The section the tab was last left on. */
  section: SettingsSection
}

export function SettingsTab({ project, section }: SettingsTabProps) {
  const [active, setActive] = useState<SettingsSection>(section)
  const settings = useSettings()
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
   * `cliSupport` degrades to a verdict-free value rather than throwing, so a build without the
   * command still draws.
   */
  // `null` until bootstrap resolves. Read as `undefined` rather than defaulted to `'claude'`,
  // so the first probe waits for the real value instead of firing once for the default and
  // again for whatever the user actually configured.
  const configuredBinary = settings?.claude.cli.binary
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
  }, [configuredBinary])

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
              claudeVersion,
              cliSupport,
              openLogDir: revealLogDir,
              logDir,
            })
          )}
        </div>
      </div>
    </div>
  )
}
