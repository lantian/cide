/**
 * The Settings screen in a shell that has **no project open**. (M74)
 *
 * # What was wrong
 *
 * The rail's ⚙ called `settingsApi.openTab(activeProjectId)` behind an `if (activeProjectId)`,
 * so with nothing open it did nothing at all — no tab, no notice, no log line. The button was
 * drawn, took a click, and was inert. `chrome/sidebarView.ts` records the rule this breaks: a
 * dead control is worse than either a real panel or an absent one.
 *
 * The cause is not a missing guard, it is where a Settings tab lives. `tab_open_settings` takes
 * a `ProjectId` and `TabKind::Settings` is a tab of a project, so with no project there is
 * genuinely nowhere to put one — while the settings themselves are global and always have been.
 * The tab was a container, never a scope.
 *
 * # What this is
 *
 * The same screen, in the work area the empty frame leaves blank, with a strip above it that
 * says what it is and offers the way out. The gesture is deliberately identical to the one with
 * a project open: ⚙ shows it — pressing again re-shows it, exactly as opening an existing tab
 * re-activates it rather than toggling it shut — and the `×` closes it, exactly as a tab's `×`
 * does. That symmetry is why ⚙ does **not** light up here: it did not light before (see
 * `selectView`, and the user report quoted in it), and a rail button that lights only when the
 * window happens to have no project would be a third behaviour for one control.
 *
 * `SettingsTab` is rendered with `project={null}`, which is a real state rather than a stub —
 * see its `project` prop for the three things that follow from it.
 */
import type { SettingsSection } from '@/ipc/client'
import { IconButton } from '@/kit/components/Button'
import { SettingsTab } from './SettingsTab'
import styles from './SettingsFrame.module.css'

export interface SettingsFrameProps {
  /**
   * Which section to open on — already resolved by the caller, which reads `lastFrameSection()`
   * when the gesture did not name one.
   */
  section: SettingsSection
  /** Put the empty work area back. The `×`, and nothing else raises it. */
  onClose: () => void
}

export function SettingsFrame({ section, onClose }: SettingsFrameProps) {
  return (
    <div className={styles.frame} data-audit="settingsFrame">
      {/*
       * A strip, not a tab strip: there is one thing here and it cannot be switched away from,
       * so it claims no `tablist` and no `tab` — `chrome/ActivityRail.tsx` makes the same
       * distinction for its tool-window toggle. It is the tab strip's height and border so the
       * screen below starts where a tab's content would.
       */}
      <div className={styles.strip}>
        <span className={styles.title}>Settings</span>
        <div className={styles.spacer} aria-hidden="true" />
        <IconButton
          icon="x"
          label="Close settings"
          data-audit="settingsFrameClose"
          onClick={onClose}
        />
      </div>
      <div className={styles.body}>
        {/*
         * `key={section}`, and it is the whole of how a *command* that names a section reaches
         * a screen that is already up: `SettingsTab` seeds its nav from this prop with a
         * `useState`, which a changed prop does not re-seed, so `settings.keymap` on an open
         * frame would otherwise do nothing at all.
         *
         * It costs a remount, which costs one `opencode models` probe — which is why the
         * caller resolves a section-less request to the section already showing. A plain ⚙
         * therefore changes no state, remounts nothing and spawns nothing; only a request for
         * a *different* section pays.
         *
         * The nav's own clicks do not come through here. They write `SettingsTab`'s
         * module-level memory instead, so they neither remount nor are lost on a close.
         */}
        <SettingsTab key={section} project={null} section={section} />
      </div>
    </div>
  )
}
