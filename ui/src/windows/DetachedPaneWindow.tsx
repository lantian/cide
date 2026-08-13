/**
 * The whole of a `pane:<uuid>` window: one pane, and enough chrome to put it back.
 *
 * The pane is not rebuilt here. It is the same `PaneFrame` and the same `TerminalPane` the
 * shell window renders, because a detached pane is not a different kind of pane — it is the
 * same pane, shown somewhere else. A second rendering of it is how the two drift, and the
 * drift shows up as a pane that behaves differently depending on which window it is in.
 *
 * Nothing here spawns. The pane arrives carrying `pane.session`, `TerminalPane` adopts that
 * binding instead of spawning, and the bytes come from the Rust screen mirror, which is fed
 * whether or not anything is attached. A detached window that spawned its own child would
 * leave the original running with nobody watching it — for a Claude pane, a duplicated
 * conversation and a doubled bill. The guard below is that rule made load-bearing rather
 * than assumed.
 *
 * The terminal itself does not travel: each Tauri window is a separate webview and a
 * separate JavaScript realm, so this window builds its own xterm against the same
 * `SessionId`. Continuity is Rust's, not the DOM's.
 */
import type { ReactNode } from 'react'
import { WindowFrame } from '@/chrome/WindowFrame'
import { PaneFrame } from '@/layout/PaneTitleBar'
import { TerminalPane } from '@/panes/TerminalPane'
import type { Pane } from '@/ipc/client'
import styles from './DetachedPaneWindow.module.css'

export interface DetachedPaneWindowProps {
  pane: Pane
  /** The project's primary root, passed through to the pane unchanged. */
  cwd: string
  /**
   * The project this pane belongs to.
   *
   * A detached pane arrives with a live session and does not spawn, so this is belt and
   * braces — but the guard below is the only thing preventing a spawn here, and a child born
   * without `CLAUDE_CODE_SSE_PORT` would bind to whichever lockfile it happened to find.
   */
  project?: string | undefined
  /**
   * The project's roots and the open gesture, for file links in this pane's output.
   *
   * Passed through untouched, exactly like `cwd` and `project`: a detached pane is the same
   * pane, and a path printed in it means the same file it would have meant in the tab.
   */
  roots?: readonly string[] | undefined
  onOpenPath?: ((path: string, at: { line: number; column: number } | null) => void) | undefined
  /** Put the pane back in its home tab and close this window. */
  onRedock?: (() => void) | undefined
}

/**
 * Whether this kind runs a child, and must therefore reach this window with one already
 * running.
 *
 * Written as the list of kinds that run *nothing*, so a `PaneKind` added later falls into
 * the default and is treated as spawning. The guard below then refuses to render rather
 * than letting `TerminalPane` start a second child — the wrong answer in that direction is
 * a pane that says it has no session, and in the other it is a duplicated conversation.
 */
function needsSession(pane: Pane): boolean {
  switch (pane.kind) {
    case 'diff':
    case 'editor':
      return false
    default:
      return true
  }
}

export function DetachedPaneWindow({
  pane,
  cwd,
  project,
  roots,
  onOpenPath,
  onRedock,
}: DetachedPaneWindowProps): ReactNode {
  // A pane can only be detached after it has spawned, so this is the state that should not
  // occur rather than one a user reaches. It is still rendered rather than ignored: letting
  // `TerminalPane` fall through to a spawn would start a child whose id no window records,
  // and an unrecorded child is one nothing can resume or reap.
  const orphaned = needsSession(pane) && pane.session === null

  return (
    <WindowFrame>
      <div className={styles.window}>
        <div className={styles.header} data-audit="detachedHeader">
          {/* `title` as well as the text: the strip is narrow and the pane's title is the
              only thing naming which session this window is showing. */}
          <span className={styles.title} title={pane.title}>
            {pane.title}
          </span>

          {/* The window carries no WM decorations, so this filler is the only drag surface.
              `data-tauri-drag-region` does not inherit, which is why it sits on an empty box
              rather than on the header — on the header it would turn the button into a drag
              handle. `data-window-drag` is what `WindowFrame` binds double-click-to-maximize
              to without either file knowing about the other. */}
          <div className={styles.filler} data-tauri-drag-region data-window-drag="true" />

          {/* The only way out of this window that the app offers, deliberately: redocking is
              what returns the pane to its tab, and a window closed without it leaves a pane
              held in the project's `detached` map with a live session and nothing showing
              it. Omitting a close light does not make that unreachable — the compositor can
              still close the window — so the close path in Rust has to redock too. */}
          <button
            type="button"
            className={styles.redock}
            title="Put this pane back in its tab"
            onClick={onRedock}
          >
            Redock
          </button>
        </div>

        <div className={styles.body}>
          {/* Index 1 and focused, both unconditionally: this window holds exactly one pane,
              so the depth-first position the tree would compute is 1, and there is no second
              pane for focus to be on. */}
          <PaneFrame pane={pane} index={1} focused maximized={false}>
            {orphaned ? (
              <p className={styles.orphan}>This pane has no session. Redock it to start one.</p>
            ) : (
              <TerminalPane
                pane={pane}
                cwd={cwd}
                project={project}
                roots={roots}
                onOpenPath={onOpenPath}
              />
            )}
          </PaneFrame>
        </div>
      </div>
    </WindowFrame>
  )
}
