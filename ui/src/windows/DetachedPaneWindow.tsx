/**
 * The whole of a `pane:<uuid>` window: one pane, and enough chrome to put it back.
 *
 * The pane is not rebuilt here. It is the same `PaneFrame` and the same `TerminalPane` the
 * shell window renders, because a detached pane is not a different kind of pane — it is the
 * same pane, shown somewhere else. A second rendering of it is how the two drift, and the
 * drift shows up as a pane that behaves differently depending on which window it is in.
 *
 * Nothing here spawns *while the app is running*. The pane arrives carrying `pane.session`,
 * `TerminalPane` adopts that binding instead of spawning, and the bytes come from the Rust
 * screen mirror, which is fed whether or not anything is attached. A detached window that
 * spawned its own child would leave the original running with nobody watching it — for a Claude
 * pane, a duplicated conversation and a doubled bill. The guard below is that rule made
 * load-bearing rather than assumed.
 *
 * After a **restart** it is the other way round and always was: this window is rebuilt from
 * `workspace.json` before any process exists, so the id it arrives with names a child that died
 * with the previous run and there is nothing to adopt. That is what `restore` is for, and its
 * absence here is what made every torn-out pane come back saying `— no such session —`.
 *
 * The terminal itself does not travel: each Tauri window is a separate webview and a
 * separate JavaScript realm, so this window builds its own xterm against the same
 * `SessionId`. Continuity is Rust's, not the DOM's.
 */
import type { ReactNode } from 'react'
import { WindowFrame } from '@/chrome/WindowFrame'
import { PaneFrame } from '@/layout/PaneTitleBar'
import { TerminalPane } from '@/panes/TerminalPane'
import type { Pane, PaneRestore } from '@/ipc/client'
import { detachedContent } from './detachedPane'
import styles from './DetachedPaneWindow.module.css'

export interface DetachedPaneWindowProps {
  pane: Pane
  /**
   * Record the session this pane bound, exactly as the shell window does.
   *
   * Omitting it is not a smaller version of the shell's behaviour, it is a leak. A pane that
   * respawns here — which is what the restart affordance does after a double Ctrl+C — mints a
   * new `SessionId`, and with nothing to report it the workspace goes on naming the child that
   * died. Re-docking then restores the dead id, so the pane comes back showing a session that
   * does not exist while the live `claude` belongs to no pane at all and survives until quit.
   *
   * Optional because the audit driver mounts this window without a workspace to write back to.
   */
  onSessionBound?: ((session: string) => void) | undefined
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
  /**
   * This pane's entry in the launch plan, when the workspace was restored.
   *
   * The paragraph above says nothing here spawns, and that is true of a pane torn out during
   * *this* run. It is not true after a restart: the window is re-created from `workspace.json`
   * before any process exists, so the pane arrives carrying a `SessionId` whose owner died with
   * the last run and there is nothing to attach to. `lifecycle::plan_restore` has always walked
   * `project.detached` and produced an entry for exactly this case — and this window never
   * passed one, so the pane adopted the dead id and printed `— no such session —` into a blank
   * pane on every launch, deterministically, for as long as detach has existed.
   */
  restore?: PaneRestore | undefined
  /** Put the pane back in its home tab and close this window. */
  onRedock?: (() => void) | undefined
}

export function DetachedPaneWindow({
  pane,
  cwd,
  project,
  roots,
  restore,
  onOpenPath,
  onSessionBound,
  onRedock,
}: DetachedPaneWindowProps): ReactNode {
  /*
   * What goes inside the frame — a decision, so it lives in `detachedPane.ts` where a check
   * script can run it.
   *
   * It was two lines here and one of them was missing. `needsSession` answered `false` for an
   * `editor` pane, which is true and was then used as *"so it is not orphaned"* — leaving
   * `TerminalPane` as the only remaining branch. A detached editor pane would have spawned a
   * shell in a window titled `main.rs`, and only `layout::take_pane`'s refusal to detach the
   * last pane of a tab kept it out of reach.
   *
   * A pane that runs a child can only be detached after it has spawned, so `orphaned` is a
   * state that should not occur rather than one a user reaches. It is still rendered rather
   * than ignored: letting `TerminalPane` fall through to a spawn would start a child whose id
   * no window records, and an unrecorded child is one nothing can resume or reap.
   */
  const content = detachedContent(pane)

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
            {content.kind === 'terminal' ? (
              <TerminalPane
                pane={pane}
                cwd={cwd}
                project={project}
                roots={roots}
                restore={restore}
                onOpenPath={onOpenPath}
                onSessionBound={onSessionBound}
              />
            ) : (
              // One element for both refusals: they differ only in their sentence, and a second
              // paragraph style is how the two drift apart.
              <p className={styles.orphan}>{content.message}</p>
            )}
          </PaneFrame>
        </div>
      </div>
    </WindowFrame>
  )
}
