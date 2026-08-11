/**
 * What goes inside a pane frame.
 *
 * Every pane is one of three things, and the distinction only exists after a restart:
 *
 * * a live terminal, which is every pane during normal use;
 * * a Claude pane whose conversation is resumable but which has not been asked to resume —
 *   it shows a splash instead of spawning, because reopening a six-pane project must not
 *   silently start six agents at once;
 * * a restored shell, which respawns at once and says in its own transcript what it did or
 *   did not get back. That notice is written by Rust into the terminal, not rendered here:
 *   as a DOM sibling above the terminal it pushed the terminal out of the pane frame and
 *   painted over the next row's title bar.
 *
 * Deciding this here rather than inside `TerminalPane` keeps that component about wiring a
 * PTY to a terminal, with no opinion about whether one should exist yet.
 */
import { useState, type ReactNode } from 'react'
import { TerminalPane } from './TerminalPane'
import { EditorPane } from './EditorPane'
import { ClaudeDiffPane } from './ClaudeDiffPane'
import { ResumeSplash } from '@/windows/ResumeSplash'
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
  // Every hook first, unconditionally, before the dispatch below returns for anything.
  //
  // This used to sit *after* the editor and diff early returns, which is a rules-of-hooks
  // violation: React identifies a hook by its call order within a component, so a render
  // that returns early declares fewer hooks than one that does not, and every later hook
  // shifts by one. It was harmless only because a pane's kind is fixed for the life of a
  // mount — a property of today's callers, not a guarantee any of them promise, and one
  // React's own lint will not accept regardless.
  //
  // Hoisting is free here. The initialiser is a pure read of the host registry, and for the
  // two document cases `restore` is `undefined` anyway — `lifecycle::entry_for` plans an
  // entry only for a Claude or Shell pane — so `held` computes to `false` and is never
  // consulted. What is *not* free is reordering these two calls relative to each other, or
  // moving them below the splash branch that reads them.

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
  // Latched on the first render rather than recomputed. `paneSessionId` flips to defined the
  // moment the child spawns, which is a render or two after this pane decided what to show —
  // and the splash and the terminal are different element *types* in the same position, so a
  // `<ResumeSplash/>` becoming a `<TerminalPane/>` on its own would unmount and re-mount
  // whatever React had just put there.
  //
  // This used to matter for shells too, when a restored one rendered as `<>banner +
  // terminal</>`: the fragment collapsing to a bare `<TerminalPane/>` tore down a terminal
  // that had only just attached, and the banner was on screen for one frame — not long enough
  // to read the only notice the user got. Both problems went away with the banner, which is
  // now a line inside the transcript rather than a DOM sibling; the latch stays for the
  // splash, which genuinely does swap element types.
  const [held] = useState(
    () => restore !== undefined && !restore.eager && paneSessionId(pane.id) === undefined,
  )
  const [resumed, setResumed] = useState(false)

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

  if (held && !resumed && pane.kind !== 'shell') {
    // A shell has no conversation to resume, so it is never held: it falls through to the
    // terminal below and respawns immediately, and its notice — "restored, and here is or is
    // not the output from last time" — is written **into the terminal** by Rust, at the top of
    // the transcript, the way `— exited —` is written at the bottom.
    //
    // It used to be a `<RestoredShellBanner/>` rendered as a sibling above `<TerminalPane/>`,
    // and that is what broke the pane. `PaneTitleBar.module.css` gives `.body` a definite
    // height and `PaneSlot` claims `height: 100%` of it, so a sibling ~31px tall pushed the
    // terminal 31px past the bottom of the frame — and the terminal's own background (white,
    // on the default theme) painted over the title bar of the row below. The reported symptom
    // was exactly that. A line inside the transcript cannot do it, scrolls away with the text
    // it describes, and survives a re-dock like every other byte in the pane.
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
