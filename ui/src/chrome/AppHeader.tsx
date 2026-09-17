/**
 * The 34px app header: traffic lights, one tab per open project, the row controls and the
 * window actions.
 *
 * The window has no WM decorations, so this bar is the whole title bar — it is also the
 * only place a project can be switched, closed or opened.
 *
 * Everything the layout audit measures is still pure: its four header measurements (`header`,
 * `projectTab`, `projectDot`, `projectPath`) are rendered from the props below, all of which
 * `App.tsx` supplies, so the component can be measured against the mock from a fixture.
 *
 * It is no longer store-*free*, and the note that used to claim so was wrong the moment the
 * context menu landed: `useContextMenu` subscribes to the workspace store for the live keymap
 * its shortcut chips come from. Two of this file's children — `ProjectMenu` and `RowControls`
 * — go further and drive the store directly, and each says at length why; neither is measured,
 * and neither replaces a prop a host was already passing.
 */
import { useEffect, useRef } from 'react'
import { useContextMenu } from '@/menus'
import { projectMenu } from '@/ipc/client'
import type { Pane as GeneratedPane, SplitIntent, Tab as GeneratedTab } from '@/ipc/generated'
import { useAwaitingInProject } from '@/panes/awaiting'
import { awaitingBadge, awaitingHint } from '@/panes/awaitingRule'
import { projectTabEntries } from './menuModel'
import { ProjectMenu } from './ProjectMenu'
import { RowControls } from './RowControls'
import { currentUserAgent, windowControlLayout } from './windowControls'
import { toggleMaximize } from './WindowFrame'
import { Icon } from '@/icons/Icon'

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
  /**
   * The project's tabs and its torn-out panes, for the *waiting for you* count.
   *
   * **Optional, and that is what keeps the measurement fixture cheap**: `chrome/layoutAudit.ts`
   * measures this header from a handful of fields, and a required `tabs` would make every
   * fixture build a whole pane tree to take a ruler to a 34px bar. Absent means "nothing
   * known", which renders as no badge — the correct answer for a fixture, and unreachable in
   * the app, where `App.tsx` passes the generated `Project`.
   *
   * Typed as the generated shapes rather than a narrowed local one because the count reaches
   * through `tree.panes` to the session on each pane; a hand-written subset here would be a
   * second declaration of `Pane` to keep in step with Rust.
   */
  tabs?: GeneratedTab[] | undefined
  detached?: Record<string, GeneratedPane> | undefined
}

export interface AppHeaderProps {
  /**
   * `readonly` because the host hands over the answer to a rule rather than an array of its
   * own: `windows/windowTabs.ts`'s `shownProjects` returns the projects THIS window draws, and
   * a `readonly` return is what stops a caller mutating a list the rule owns. The header only
   * ever maps and finds over it.
   */
  projects?: readonly ProjectTab[] | undefined
  activeProject?: string | null | undefined
  onActivate?: ((id: string) => void) | undefined
  onClose?: ((id: string) => void) | undefined
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
  const tabsRef = useRef<HTMLDivElement>(null)

  // Bring the active project's tab into view when it changes.
  //
  // Without this the recents menu is half a feature: picking a project past the edge of the
  // strip activates it and leaves its tab off-screen, so the header still shows some other
  // project as the one you are in. `block: 'nearest'` so a tab already visible does not
  // scroll, and `inline: 'nearest'` so reaching one just past the edge moves by the least it
  // can rather than centring and shuffling the whole strip.
  useEffect(() => {
    const strip = tabsRef.current
    if (!strip) return
    const tab = strip.querySelector('[data-active="true"]')
    tab?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
  }, [activeProject])

  const tabMenu = useContextMenu({
    label: 'Project tab',
    items: ({ target }) => {
      const id = target?.closest<HTMLElement>('[data-project-id]')?.dataset.projectId
      const project = projects.find((p) => p.id === id)
      // A right-click on the strip's own background rather than on a tab. Nothing to offer, so
      // the hook declines to open rather than showing a box of lines about no project.
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
      {CONTROLS.side === 'left' ? <WindowLights /> : null}

      <div
        ref={tabsRef}
        className={styles.tabs}
        onContextMenu={tabMenu.onContextMenu}
        // A vertical wheel over a horizontally-scrolling box does nothing on Linux, so the
        // strip would scroll only by dragging a scrollbar that is deliberately hidden. Mapped
        // here rather than in CSS because there is no CSS for it.
        //
        // `deltaX` is preferred when the device reports one (a touchpad's horizontal gesture,
        // or a tilt wheel); an ordinary wheel reports only `deltaY` and that is what is
        // borrowed. Not `preventDefault`: the header has nothing else to scroll, so letting
        // the event through costs nothing and swallowing it would break a browser default
        // nobody asked us to own.
        onWheel={(e) => {
          const strip = tabsRef.current
          if (!strip || strip.scrollWidth <= strip.clientWidth) return
          strip.scrollLeft += e.deltaX !== 0 ? e.deltaX : e.deltaY
        }}
      >
        {projects.map((project) => (
          <ProjectTabItem
            key={project.id}
            project={project}
            active={project.id === activeProject}
            onActivate={onActivate}
            onClose={onClose}
          />
        ))}
      </div>

      {/*
       * The `+` and the recent-projects caret. A component of its own because the `+` had to
       * stop going through `@tauri-apps/plugin-dialog` — its dialog cannot be parented on
       * Linux and so opened behind this window. The header takes no handler for it at all:
       * the prop that used to be accepted-and-ignored is gone, and `chrome/projectOpen.ts` is
       * now the one road, shared with the palette's *Open project…*.
       *
       * **A sibling of `.tabs`, not a child of it.** It used to sit inside, after the last
       * project, and `.tabs` is a clipping box full of `flex: none` tabs — so at seven or eight
       * open projects the only control that can open a ninth scrolled out of the window and
       * became unreachable. Outside, it is pinned: the strip shrinks and clips its own last tab
       * instead, which is the behaviour that stylesheet already documents for tabs and the same
       * argument `RowControls` below makes for itself.
       */}
      <ProjectMenu />

      {/* `data-tauri-drag-region` does not inherit, so the drag surface is this filler rather
          than the header itself. Also a sibling now: it is what takes the slack `.tabs` gave
          up when it stopped being `flex: 1`, so the two controls above stay hard against the
          project strip instead of being pushed to the far right. `data-window-drag` is the hook
          the window-frame agent binds double-click-to-maximize to. */}
      <div className={styles.filler} data-tauri-drag-region data-window-drag="true" />

      {/* The two row gestures, moved up out of `SplitTree`'s bottom strip on request. Outside
          `.tabs` so an overfull project strip clips its own last tab rather than pushing these
          off the window edge — the same argument `TabStrip` makes about its own boxes. */}
      <RowControls onAddRow={onAddRow} />

      <div className={styles.actions}>
        <button type="button" className={styles.action} title="Toggle theme" onClick={onToggleTheme}>
          <Icon name="sun-moon" size={1} />
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
          <Icon name="columns-2" size={1} />
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
          <Icon name="picture-in-picture-2" size={1} />
        </button>
      </div>

      {CONTROLS.side === 'right' ? <WindowLights /> : null}

      {/* Rendered or nothing appears. It portals out of this bar, so the header's own
          `overflow: hidden` on `.tabs` cannot clip it. */}
      {tabMenu.menu}
    </div>
  )
}

/*
 * Resolved once, at module load, rather than per render: the platform cannot change while the
 * app is running, and a value that is stable by construction cannot make the buttons jump
 * between sides on a re-render.
 */
const CONTROLS = windowControlLayout(currentUserAgent())

/** Per-button styling and label, keyed by the id `WindowFrame.tsx` binds behaviour to. */
const LIGHTS = {
  close: { variant: 'close', title: 'Close window' },
  minimize: { variant: 'minimize', title: 'Minimize window' },
  zoom: { variant: 'zoom', title: 'Zoom window' },
} as const

/**
 * Close, minimize and zoom, on the side this platform puts them.
 *
 * Window behaviour belongs to the window-frame agent. These carry `data-window-button` so it
 * can bind close/minimize/zoom without this file knowing about `@tauri-apps/api`, which only
 * `ipc/client.ts` may import.
 *
 * Rendered by the header at one end or the other — `windowControls.ts` decides which, and
 * gives the paint order with it, because close belongs at the window's outer corner on both
 * layouts and that is a different end of the cluster on each. One component invoked from two
 * places rather than two copies: the three buttons and their audit hooks are the same markup
 * either way, and the layout audit's `trafficLight` measurements would otherwise have two
 * sources to drift between.
 *
 * Exported for `windows/DetachedTabHeader.tsx`, which is the same argument one window over:
 * a torn-out tab's header needs the same three lights, and a second copy of this markup is
 * how one of the two stops matching the `data-window-button` contract.
 */
export function WindowLights() {
  return (
    <div className={`${styles.lights} ${CONTROLS.side === 'right' ? styles.lightsRight : ''}`}>
      {CONTROLS.order.map((id) => (
        <button
          key={id}
          type="button"
          className={`${styles.light} ${styles[LIGHTS[id].variant]}`}
          data-window-button={id}
          data-audit="trafficLight"
          title={LIGHTS[id].title}
        />
      ))}
    </div>
  )
}

interface ProjectTabItemProps {
  project: ProjectTab
  active: boolean
  onActivate?: ((id: string) => void) | undefined
  onClose?: ((id: string) => void) | undefined
}

/**
 * One project's tab, and the only place a *background* project can say it wants attention.
 *
 * # Why the tab is its own component now
 *
 * Because it holds a hook. `useAwaitingInProject` subscribes per project, so the snapshot is a
 * number and `Object.is` re-renders one tab when one project's count moves — six open projects
 * and one `Stop` hook is one re-render, not six. A hook cannot be called inside the `.map` that
 * used to build these, and hoisting the whole strip's counts into one object handed down as a
 * prop would be a new identity on every store notification and a re-render of the entire
 * header for ever. `TabStrip`'s `TabItem` is the same component for the same reason.
 *
 * # Why the header carries this at all
 *
 * `App.tsx` renders the tab strip and the pane tree for the **active project only**. Every
 * other project's panes and tabs are not mounted, so their markers and badges are painted
 * nowhere — while Rust has already put `Awaiting: N` in the OS title, counting them. That gap
 * is the whole of the reported problem: the task bar said something was waiting and no surface
 * in the window said what. This tab is the only element of a background project that exists on
 * screen, so it is the only place the answer can go.
 *
 * # The badge, not a recoloured dot
 *
 * The project dot is 8×8 and its geometry is pinned by the layout audit; it also already means
 * something (which project this is), and overloading it would make "waiting" and "this is the
 * atlas project" the same pixel. A separate box carries a *count*, for the same reason the tab
 * strip's does: a dot says "something behind here" and leaves the user to hunt, a number says
 * how many and decrements as each pane is dealt with, which is the only progress signal
 * available while those panes are not rendered at all.
 *
 * Its box is rendered on every tab but sized only when filled — the stylesheet collapses the
 * empty box via `data-awaiting` on the row, and `TabStrip.module.css` records why the
 * permanent reserve this replaced was the wrong trade.
 */
/**
 * Double-clicking a project tab maximizes the window, exactly as double-clicking the header's
 * empty filler does.
 *
 * The report was that the two behave differently, and they did: `WindowFrame`'s delegated
 * `dblclick` listener bails unless `target.closest('[data-window-drag="true"]')` finds
 * something, and the only element in the header carrying that attribute is `.filler` — a
 * *sibling* of the tab strip, deliberately, because putting it on the header would turn every
 * tab and button into a drag handle.
 *
 * An explicit handler rather than marking the row as a drag region. The delegated path would
 * in fact reach `toggleMaximize` from here, but only because `tauriOwnsDoubleClick` answers
 * `false` when a `<button>` sits between the target and the region — a rule that exists to
 * mirror Tauri's injected `drag.js`, not to decide anything about tabs, and it should not
 * quietly become load-bearing for this gesture. Nothing here carries
 * `data-tauri-drag-region`, so `drag.js` never sees this double-click and there is no
 * double-toggle to guard against.
 *
 * Importing `toggleMaximize` is not a hole in the "only `ipc/client.ts` touches
 * `@tauri-apps/api`" rule: `WindowFrame.tsx` is the documented owner of window controls and
 * already exports this for exactly this kind of caller.
 *
 * The close button is excluded. Its first click closes the project, so the second lands on
 * whatever reflowed into that spot — and maximizing the window as a parting gift for closing
 * something is not what anybody meant.
 */
function onTabDoubleClick(event: React.MouseEvent<HTMLDivElement>): void {
  const target = event.target
  if (target instanceof Element && target.closest(`.${styles.tabClose}`) !== null) return
  void toggleMaximize()
}

function ProjectTabItem({ project, active, onActivate, onClose }: ProjectTabItemProps) {
  // `tabs`/`detached` are optional on `ProjectTab` so a four-field measurement fixture still
  // satisfies it, and a header rendered from one simply has nothing waiting. `App.tsx` passes
  // the generated `Project` straight through, which carries both.
  const waiting = useAwaitingInProject(project)
  const badge = awaitingBadge(waiting)
  const hint = awaitingHint(waiting, 'project')

  return (
    <div
      className={active ? `${styles.tab} ${styles.tabActive}` : styles.tab}
      data-audit="projectTab"
      // How the context menu finds which tab was hit; `items` runs at open time and
      // reads it back off the DOM rather than one hook closing over each project.
      data-project-id={project.id}
      data-active={active ? 'true' : 'false'}
      data-awaiting={waiting > 0 ? String(waiting) : 'false'}
      onDoubleClick={onTabDoubleClick}
    >
      <button
        type="button"
        className={styles.tabOpen}
        // The badge is `pointer-events: none` so the tab stays one box the pointer can hit
        // anywhere, so it cannot carry its own tooltip — the tab's does, and it is also the
        // only place a user with eleven waiting sessions learns that `9+` means eleven.
        title={hint ? `${project.displayPath} — ${hint}` : project.displayPath}
        // Which project is open is otherwise carried by colour and a 2px rule alone,
        // neither of which a screen reader reports.
        aria-current={active}
        onClick={() => onActivate?.(project.id)}
      >
        <span className={styles.dot} style={{ background: project.dot }} data-audit="projectDot" />
        <span>{project.name}</span>
        {/* The path is the active tab's alone: the mock keeps inactive tabs to a
            name so a full strip still fits. */}
        {active && (
          <span className={styles.path} data-audit="projectPath">
            {project.displayPath}
          </span>
        )}
      </button>
      {/*
       * Inside `.tabOpen`'s row but after it, so the count sits between the name and the close
       * button and clicking it still activates the project underneath.
       *
       * `role` is on the empty box too, and the reason is the same one `TabStrip` states: a
       * live region that comes into existence at the same moment as its content is not
       * reliably announced, because nothing was there to be watching. The region exists from
       * mount and `aria-hidden` is what flips.
       */}
      <span
        className={badge ? `${styles.awaiting} ${styles.awaitingOn}` : styles.awaiting}
        role="status"
        aria-label={hint}
        aria-hidden={badge ? undefined : true}
      >
        {badge}
      </span>
      <button
        type="button"
        className={styles.tabClose}
        title={`Close ${project.name}`}
        onClick={() => onClose?.(project.id)}
      >
        <Icon name="x" size={1} />
      </button>
    </div>
  )
}
