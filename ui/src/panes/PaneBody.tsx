/**
 * What goes inside a pane frame.
 *
 * Every pane is one of three things, and the distinction only exists after a restart:
 *
 * * a live terminal, which is every pane during normal use;
 * * a Claude pane whose conversation is resumable but which has not been asked to resume —
 *   it shows a splash instead of spawning, because reopening a six-pane project must not
 *   silently start six agents at once;
 * * a shell that came back with nothing, which says so rather than presenting an empty
 *   terminal as though nothing were missing.
 *
 * Deciding this here rather than inside `TerminalPane` keeps that component about wiring a
 * PTY to a terminal, with no opinion about whether one should exist yet.
 */
import { useState, type ReactNode } from 'react'
import { TerminalPane } from './TerminalPane'
import { ResumeSplash, RestoredShellBanner } from '@/windows/ResumeSplash'
import type { Pane, PaneRestore } from '@/ipc/client'

export interface PaneBodyProps {
  pane: Pane
  cwd: string
  /**
   * This pane's entry in the launch plan, when the workspace was restored.
   *
   * Absent for a pane created during the session — those always spawn, because the user
   * just asked for them.
   */
  restore?: PaneRestore | undefined
  onSessionBound?: ((session: string) => void) | undefined
}

export function PaneBody({ pane, cwd, restore, onSessionBound }: PaneBodyProps): ReactNode {
  // A pane that is already bound to a session has nothing to decide: the child is running
  // and the terminal attaches to it.
  const bound = pane.session !== null

  // Only a plan entry can hold a pane back, and only before it has been asked.
  const held = restore !== undefined && !restore.eager && !bound
  const [resumed, setResumed] = useState(false)

  if (held && !resumed) {
    // A shell has no conversation to resume. Its scrollback is genuinely gone, so it
    // respawns immediately with a line saying as much rather than offering a choice that
    // would restore nothing.
    if (pane.kind === 'shell') {
      return (
        <>
          <RestoredShellBanner />
          <TerminalPane pane={pane} cwd={cwd} onSessionBound={onSessionBound} />
        </>
      )
    }

    return (
      <ResumeSplash
        title={pane.title}
        lastActive={null}
        cwd={cwd}
        onResume={() => setResumed(true)}
      />
    )
  }

  return <TerminalPane pane={pane} cwd={cwd} onSessionBound={onSessionBound} />
}
