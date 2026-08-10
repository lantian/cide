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
import { EditorPane } from './EditorPane'
import { ClaudeDiffPane } from './ClaudeDiffPane'
import { ResumeSplash, RestoredShellBanner } from '@/windows/ResumeSplash'
import { paneSessionId } from '@/layout/paneHosts'
import type { DiffSpec, Pane, PaneRestore } from '@/ipc/client'

export interface PaneBodyProps {
  /** The project this pane belongs to, so its child reaches the right IDE server. */
  project?: string | undefined
  /** The project's primary session — what a `forkPrimary` split branches from. */
  primarySession?: string | undefined
  pane: Pane
  cwd: string
  /**
   * Set when this pane's tab is a diff, which makes it a document rather than a process.
   *
   * A `ClaudeMcp` diff additionally holds an agent turn open, so the pane it renders is the
   * only thing standing between the model and an answer.
   */
  diff?: DiffSpec | undefined
  /**
   * Set when this pane's tab is a file, which makes it a document rather than a process.
   *
   * Deliberately the same shape as `diff` above: both say "this tab is not a terminal, here
   * is what it is instead", and the tab id travels with the path because the editor reports
   * the dirty dot back to the tab that draws it. (M9)
   */
  editor?: { tab: string; path: string } | undefined
  /**
   * This pane's entry in the launch plan, when the workspace was restored.
   *
   * Absent for a pane created during the session — those always spawn, because the user
   * just asked for them.
   */
  restore?: PaneRestore | undefined
  onSessionBound?: ((session: string) => void) | undefined
}

export function PaneBody({
  pane,
  cwd,
  project,
  primarySession,
  diff,
  editor,
  restore,
  onSessionBound,
}: PaneBodyProps): ReactNode {
  // A file is a document too: no session, no spawn, nothing to resume. (M9)
  //
  // Dispatched on the PANE kind, not the tab kind. Splitting a File tab creates a Claude
  // pane — `default_intent` says so — and keying this on the tab would render a second
  // independent `EditorPane` over the same path in it. Two editors over one file, each with
  // its own CodeMirror state and neither aware of the other, against a single tab-level
  // dirty flag: saving in one would clear the close guard while the other still held
  // unsaved text. Keying on the pane means a File tab has exactly one editor, which is what
  // makes one dirty flag per tab the right shape rather than a race.
  if (editor && pane.kind === 'editor') {
    return <EditorPane path={editor.path} root={cwd} project={project} tab={editor.tab} />
  }

  // A diff is a document, not a process: it never spawns and has no session to adopt. The
  // `ClaudeMcp` case is the one that matters, because it is holding an agent turn open.
  if (diff && diff.origin.kind === 'claudeMcp' && project) {
    return (
      <ClaudeDiffPane
        project={project}
        requestId={diff.origin.requestId}
        oldPath={diff.oldPath}
        newPath={diff.newPath}
      />
    )
  }

  // Whether this pane is held at a splash instead of spawning.
  //
  // A pane that already has a *running* child has nothing to decide: the terminal attaches
  // to it and there is nothing to resume.
  //
  // Asked of the host registry, not of `pane.session`. That field is what the domain saved
  // last time and it is set on every restored pane, so the predicate used to read
  // `!bound` — and a restored pane is bound by definition, which made the Resume splash
  // unreachable and every non-eager pane spawn silently on launch. That is precisely what
  // `eager` exists to prevent: reopening a six-pane project started six agents.
  //
  // `paneSessionId` also survives host eviction, so a pane the user resumed and then left
  // parked in another tab does not come back offering to resume itself a second time.
  //
  // Latched on the first render rather than recomputed, and that is load-bearing twice over.
  // `paneSessionId` flips to defined the moment the child spawns, which is a render or two
  // after this pane decided to show it — and the two branches below are different element
  // *types* in the same position, so a `<>banner + terminal</>` becoming a bare
  // `<TerminalPane/>` makes React unmount the terminal it mounted a moment ago, tear down its
  // sink and re-attach. It also meant the restored-shell banner was on screen only for the
  // frame between the spawn resolving and the domain recording the binding, which is not long
  // enough to read — a line that says "previous output not retained" is the only notice the
  // user gets that a shell came back empty.
  //
  // The alternative that lost was giving both branches the same element shape (always a
  // fragment, with a `null` where the banner is not wanted). That stops the remount but still
  // blinks the banner away, because the condition itself is what is unstable.
  const [held] = useState(
    () => restore !== undefined && !restore.eager && paneSessionId(pane.id) === undefined,
  )
  const [resumed, setResumed] = useState(false)

  if (held && !resumed) {
    // A shell has no conversation to resume. Its scrollback is genuinely gone, so it
    // respawns immediately with a line saying as much rather than offering a choice that
    // would restore nothing.
    if (pane.kind === 'shell') {
      return (
        <>
          <RestoredShellBanner />
          <TerminalPane
            pane={pane}
            cwd={cwd}
            project={project}
            primarySession={primarySession}
            restore={restore}
            onSessionBound={onSessionBound}
          />
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

  // `restore` travels on, always: it is what tells `specFor` to pass `--resume <id>` instead
  // of adopting a `SessionId` whose process died with the last run.
  return (
    <TerminalPane
      pane={pane}
      cwd={cwd}
      project={project}
      primarySession={primarySession}
      restore={restore}
      onSessionBound={onSessionBound}
    />
  )
}
