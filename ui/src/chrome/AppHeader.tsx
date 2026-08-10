/**
 * The 34px app header: traffic lights, one tab per open project, and the window actions.
 *
 * The window has no WM decorations, so this bar is the whole title bar — it is also the
 * only place a project can be switched, closed or opened.
 *
 * Nothing here reads the store. The layout audit measures this component in isolation
 * against the mock, which it cannot do if the component reaches into global state; App.tsx
 * does the wiring.
 */
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
}

export function AppHeader({
  // No projects is a real state — first launch, and after the last project closes. It
  // renders as the bare frame with a `+`, never as an error or an empty bar.
  projects = [],
  activeProject = null,
  onActivate,
  onClose,
  onNew,
  onToggleTheme,
  onSplit,
  onDetach,
}: AppHeaderProps) {
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

      <div className={styles.tabs}>
        {projects.map((project) => {
          const active = project.id === activeProject
          return (
            <div
              key={project.id}
              className={active ? `${styles.tab} ${styles.tabActive}` : styles.tab}
              data-audit="projectTab"
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

        <button type="button" className={styles.add} title="Open project" onClick={onNew}>
          +
        </button>

        {/* `data-tauri-drag-region` does not inherit, so the drag surface is this filler
            rather than the header itself. `data-window-drag` is the hook the window-frame
            agent binds double-click-to-maximize to, without depending on this markup. */}
        <div className={styles.filler} data-tauri-drag-region data-window-drag="true" />
      </div>

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
    </div>
  )
}
