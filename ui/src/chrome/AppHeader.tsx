/**
 * The 34px app header: traffic lights, one tab per open project, the row controls and the
 * window actions.
 *
 * The window has no WM decorations, so this bar is the whole title bar — it is also the
 * only place a project can be switched, closed or opened.
 *
 * This file itself still reads no store: every prop below is supplied by `App.tsx` and the
 * layout audit's four header measurements (`header`, `projectTab`, `projectDot`,
 * `projectPath`) are all rendered from them. Two of its children — `ProjectMenu` and
 * `RowControls` — do wire themselves, and each says at length why; neither is measured, and
 * neither replaces a prop a host was already passing.
 */
import { useContextMenu } from '@/menus'
import { projectMenu } from '@/ipc/client'
import type { SplitIntent } from '@/ipc/generated'
import { projectTabEntries } from './menuModel'
import { ProjectMenu } from './ProjectMenu'
import { RowControls } from './RowControls'
import styles from './AppHeader.module.css'

/**
 * The header's slice of a project. Structurally satisfied by the generated `Project`, so
 * App.tsx can pass one straight through, while a measurement harness can supply four
 * fields instead of a whole workspace.
 */
export interface ProjectTab {
  id: string
  name: string
  /** Display form of the primary root, e.g. `~/work/cide`. */
  displayPath: string
  /** CSS colour for the tab's dot, typically a `var(--…)` reference. */
  dot: string
}

export interface AppHeaderProps {
  projects?: ProjectTab[] | undefined
  activeProject?: string | null | undefined
  onActivate?: ((id: string) => void) | undefined
  onClose?: ((id: string) => void) | undefined
  /**
   * **No longer used, and deliberately still accepted.**
   *
   * `App.tsx` supplies a handler that opens `@tauri-apps/plugin-dialog`'s folder dialog. That
   * dialog cannot be parented on Linux — the plugin guards `set_parent` behind
   * `cfg(windows | macos)` — which is exactly the reported "the project popup opens *behind*
   * the cide window". The `+` therefore goes to `project_pick` instead, unconditionally: a fix
   * that took effect only once a second file stopped passing a prop is a fix that ships off.
   *
   * The field stays so the existing call site still type-checks, and so this comment sits where
   * whoever deletes it will read it. Delete the prop and `pickProject` together.
   */
  onNew?: (() => void) | undefined
  onToggleTheme?: (() => void) | undefined
  /**
   * Split the focused pane of the active tab.
   *
   * The header's ⊞ is the same gesture `TabStrip`'s already is, aimed at whatever pane has
   * focus, so a host supplies the same call:
   * `useWorkspace`'s `splitPane(project, tab.id, tab.tree.focused, 'row', 'after')`. The
   * intent is left `null` on purpose — which pane kind a split produces is the domain's
   * decision, not the header's.
   *
   * Omitted renders the button **disabled**, not inert. There is no project, or no host that
   * can split, and a control that looks live and does nothing is indistinguishable from a
   * broken app — which is the state this header shipped in.
   */
  onSplit?: (() => void) | undefined
  /**
   * Detach the focused pane into its own window.
   *
   * A host supplies `useWorkspace`'s `detachPane(project, tab.id, tab.tree.focused)`, which
   * measures the pane and forwards to `window_detach_pane`. Same disabled-when-absent
   * contract as `onSplit`.
   */
  onDetach?: (() => void) | undefined
  /**
   * Add a full-width row holding one pane, anchored under the focused one.
   *
   * **Optional in the strong sense**: omitted, the controls still work — `RowControls` drives
   * `useWorkspace.addRow` itself. This exists so a fixture can intercept the gesture, and so a
   * host that wants a different anchor can supply one. See `RowControls.tsx` for why the
   * fallback is there rather than a fifth prop nobody remembers to pass.
   */
  onAddRow?: ((intent: SplitIntent) => void) | undefined
}

export function AppHeader({
  // No projects is a real state — first launch, and after the last project closes. It
  // renders as the bare frame with a `+`, never as an error or an empty bar.
  projects = [],
  activeProject = null,
  onActivate,
  onClose,
  onToggleTheme,
  onSplit,
  onDetach,
  onAddRow,
}: AppHeaderProps) {
  /*
   * The project tab menu.
   *
   * `items` is called at open time, so which tab was hit is read off the DOM rather than
   * captured per tab — one hook for the whole strip instead of one per project, which is what
   * keeps the cost independent of how many are open.
   */
  const tabMenu = useContextMenu({
    label: 'Project tab',
    items: ({ target }) => {
      const id = target?.closest<HTMLElement>('[data-project-id]')?.dataset.projectId
      const project = projects.find((p) => p.id === id)
      // A right-click on the drag filler or the `+`, not on a tab. Nothing to offer, so the
      // hook declines to open rather than showing a box of lines about no project.
      if (!project) return []
      const clipboard = typeof navigator === 'undefined' ? undefined : navigator.clipboard
      return projectTabEntries(projects, project, {
        close: onClose,
        // Rejects when the root has gone, and `Failures` says so. Better than a file manager
        // opening on nothing, which is what a blind `open_path` produces.
        reveal: (projectId) => void projectMenu.reveal(projectId),
        ...(clipboard ? { copy: (text: string) => void clipboard.writeText(text) } : {}),
      })
    },
  })

  // The `data-audit` attributes below are how chrome/layoutAudit.ts locates this surface:
  // CSS Module class names are hashed at build time, so nothing outside can select on them.
  return (
    <div className={styles.header} data-audit="header">
      {/*
       * Window behaviour belongs to the window-frame agent. These carry `data-window-button`
       * so it can bind close/minimize/zoom without this file knowing about `@tauri-apps/api`,
       * which only `ipc/client.ts` may import.
       */}
      <div className={styles.lights}>
        <button
          type="button"
          className={`${styles.light} ${styles.close}`}
          data-window-button="close"
          data-audit="trafficLight"
          title="Close window"
        />
        <button
          type="button"
          className={`${styles.light} ${styles.minimize}`}
          data-window-button="minimize"
          data-audit="trafficLight"
          title="Minimize window"
        />
        <button
          type="button"
          className={`${styles.light} ${styles.zoom}`}
          data-window-button="zoom"
          data-audit="trafficLight"
          title="Zoom window"
        />
      </div>

      <div className={styles.tabs} onContextMenu={tabMenu.onContextMenu}>
        {projects.map((project) => {
          const active = project.id === activeProject
          return (
            <div
              key={project.id}
              className={active ? `${styles.tab} ${styles.tabActive}` : styles.tab}
              data-audit="projectTab"
              // How the context menu finds which tab was hit; `items` runs at open time and
              // reads it back off the DOM rather than one hook closing over each project.
              data-project-id={project.id}
              data-active={active ? 'true' : 'false'}
            >
              <button
                type="button"
                className={styles.tabOpen}
                title={project.displayPath}
                // Which project is open is otherwise carried by colour and a 2px rule alone,
                // neither of which a screen reader reports.
                aria-current={active}
                onClick={() => onActivate?.(project.id)}
              >
                <span
                  className={styles.dot}
                  style={{ background: project.dot }}
                  data-audit="projectDot"
                />
                <span>{project.name}</span>
                {/* The path is the active tab's alone: the mock keeps inactive tabs to a
                    name so a full strip still fits. */}
                {active && (
                  <span className={styles.path} data-audit="projectPath">
                    {project.displayPath}
                  </span>
                )}
              </button>
              <button
                type="button"
                className={styles.tabClose}
                title={`Close ${project.name}`}
                onClick={() => onClose?.(project.id)}
              >
                ×
              </button>
            </div>
          )
        })}

        {/*
         * The `+` and the recent-projects caret. A component of its own because the `+` had to
         * stop going through `@tauri-apps/plugin-dialog` — its dialog cannot be parented on
         * Linux and so opened behind this window. `onNew` is not forwarded; see its doc above.
         */}
        <ProjectMenu />

        {/* `data-tauri-drag-region` does not inherit, so the drag surface is this filler
            rather than the header itself. `data-window-drag` is the hook the window-frame
            agent binds double-click-to-maximize to, without depending on this markup. */}
        <div className={styles.filler} data-tauri-drag-region data-window-drag="true" />
      </div>

      {/* The two row gestures, moved up out of `SplitTree`'s bottom strip on request. Outside
          `.tabs` so an overfull project strip clips its own last tab rather than pushing these
          off the window edge — the same argument `TabStrip` makes about its own boxes. */}
      <RowControls onAddRow={onAddRow} />

      <div className={styles.actions}>
        <button type="button" className={styles.action} title="Toggle theme" onClick={onToggleTheme}>
          ◐
        </button>
        {/*
         * ⊞ and ⧉ are disabled when no host supplied a handler, rather than drawn live and
         * doing nothing on click. Both spent a milestone in the latter state, which reads
         * exactly like a broken app: there is no feedback to distinguish "this window cannot
         * split right now" from "the split command crashed".
         *
         * `disabled` rather than hiding them: the mock states three actions in this corner,
         * and a control that vanishes when unavailable teaches nothing about why. The title
         * carries the reason, since the glyph cannot.
         */}
        <button
          type="button"
          className={`${styles.action} ${styles.actionSplit}`}
          title={onSplit === undefined ? 'Split pane — unavailable in this window' : 'Split pane'}
          disabled={onSplit === undefined}
          onClick={onSplit}
        >
          ⊞
        </button>
        <button
          type="button"
          className={styles.action}
          title={
            onDetach === undefined ? 'Detach window — unavailable in this window' : 'Detach window'
          }
          disabled={onDetach === undefined}
          onClick={onDetach}
        >
          ⧉
        </button>
      </div>

      {/* Rendered or nothing appears. It portals out of this bar, so the header's own
          `overflow: hidden` on `.tabs` cannot clip it. */}
      {tabMenu.menu}
    </div>
  )
}
