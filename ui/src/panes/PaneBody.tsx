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
import { ImagePane } from './ImagePane'
import { imageKindFor } from './imageKinds'
import { ClaudeDiffPane } from './ClaudeDiffPane'
// Not lazy-loaded, for the reason `ImagePane` below is not: a pane that renders nothing for a
// frame while a chunk arrives is a pane the layout measures at zero.
import { GitDiffPane } from './GitDiffPane'
import { RevisionPane } from './RevisionPane'
import { MergePane } from './MergePane'
import { ResumeSplash } from '@/windows/ResumeSplash'
import { paneSessionId } from '@/layout/paneHosts'
import { useWorkspace } from '@/store/workspace'
import type { Bootstrap, DiffSpec, Pane, PaneId, PaneRestore, TabKind } from '@/ipc/client'

/** The `TabKind::Revision` arm, so the lookup below and the pane agree about its shape. */
type RevisionTab = Extract<TabKind, { kind: 'revision' }>

/** The `TabKind::Merge` arm — the three-pane conflict resolver. (M20) */
type MergeTab = Extract<TabKind, { kind: 'merge' }>

/**
 * The revision tab this pane belongs to, or `null`.
 *
 * # Why this is read off the mirror instead of being passed in
 *
 * Every other document branch below is told what its tab is: `App.tsx` computes `diff` and
 * `editor` from `tab.kind` and hands them down. That is one shape, and a `revision` prop beside
 * them would be the obvious third — except that it would work in `App.tsx`'s call site and
 * nowhere else, and it is not the only reason to prefer this.
 *
 * `panes/diffTabs.ts` already established the pattern and its header carries the argument in
 * full: a pane that can answer a question about its own tab out of the `Workspace` that
 * `cide://workspace-changed` delivers to *every* window does not depend on one host remembering
 * to thread a prop through. This project's most repeated defect is a prop that stops one
 * component short — `check:editor` asserts that `onScreen` is passed on in two files for exactly
 * that reason — and a tab kind that silently renders as a terminal is that defect with a spawn
 * on the end of it.
 *
 * A `find` over a handful of tabs per render, on a component that already re-renders on every
 * snapshot because `App.tsx` maps the whole tree. The alternative — a `panes` index keyed by
 * `PaneId` — would be a second structure to keep in step with the tree that already holds one.
 */
function revisionTabFor(
  boot: Bootstrap | null,
  project: string | undefined,
  pane: PaneId,
): RevisionTab | null {
  if (boot === null || project === undefined) return null
  const tabs = boot.workspace.projects[project]?.tabs
  if (tabs === undefined) return null
  for (const tab of tabs) {
    if (tab.kind.kind === 'revision' && pane in tab.tree.panes) return tab.kind
  }
  return null
}

/**
 * The merge tab this pane belongs to, or `null`.
 *
 * `revisionTabFor`'s twin, and its header carries the argument for reading this off the mirror
 * rather than threading a prop.
 *
 * **Returns `tab.kind` itself, never a copy**, and the resolver's tab id is read by a *second*
 * selector below rather than folded in here. A spread — `{ ...tab.kind, id: tab.id }` — is a
 * fresh object on every store snapshot, which under `Object.is` equality re-renders this pane
 * for every unrelated workspace change. `check:selectors` exists for exactly that class and
 * does not catch it in this shape, because the fresh value is built inside a named function
 * rather than inline in the selector.
 */
function mergeTabFor(
  boot: Bootstrap | null,
  project: string | undefined,
  pane: PaneId,
): MergeTab | null {
  if (boot === null || project === undefined) return null
  const tabs = boot.workspace.projects[project]?.tabs
  if (tabs === undefined) return null
  for (const tab of tabs) {
    if (tab.kind.kind === 'merge' && pane in tab.tree.panes) return tab.kind
  }
  return null
}

/**
 * The id of the merge tab this pane belongs to — a plain string, so it compares by value.
 *
 * Split from `mergeTabFor` for the reason that function's header gives. The resolver needs it
 * because it closes its own tab once the file is written, and a pane cannot ask the workspace
 * which tab contains it from the inside.
 */
function mergeTabIdFor(
  boot: Bootstrap | null,
  project: string | undefined,
  pane: PaneId,
): string | null {
  if (boot === null || project === undefined) return null
  const tabs = boot.workspace.projects[project]?.tabs
  if (tabs === undefined) return null
  for (const tab of tabs) {
    if (tab.kind.kind === 'merge' && pane in tab.tree.panes) return tab.id
  }
  return null
}

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
  /** The project's roots and the open gesture, for file links in terminal output. */
  roots?: readonly string[] | undefined
  onOpenPath?: ((path: string, at: { line: number; column: number } | null) => void) | undefined
  /** Ctrl+click on a directory. Absent in a window with no file tree — see `TerminalPane`. */
  onRevealPath?: ((path: string) => void) | undefined
  /**
   * Whether this pane's **tab** is the one in front. (M16)
   *
   * Only the editor branch reads it, and only for the status bar's single slot — a hidden tab is
   * `visibility: hidden` rather than unmounted, so its panes are mounted, measured and painting,
   * and nothing below this can tell them from the visible ones. `TabContent` has passed the flag
   * to its `renderTree` since M4 for exactly this class of consumer; `App.tsx` discarded it.
   *
   * A terminal pane ignores it: the WebGL pool answers the same question its own way, through
   * `paneHosts`, and a second route to it would be a second answer.
   */
  onScreen?: boolean | undefined
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
  roots,
  onOpenPath,
  onRevealPath,
  onSessionBound,
  onScreen,
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

  /*
   * Is this pane's tab a `TabKind::Revision`? (M18)
   *
   * A hook, so it belongs up here with the other two rather than beside the branch that uses it
   * — see the paragraph above about hook order, which is the rule that makes this placement
   * non-negotiable rather than tidy.
   *
   * The selector returns the tab's `kind` object, which is reference-stable within a snapshot
   * and replaced wholesale by the next one; that costs nothing, because `App.tsx` maps the whole
   * pane tree off the same snapshot and this component re-renders with it either way. What it
   * must not do is build a new object per call — zustand compares with `Object.is` — so the
   * lookup returns the mirror's own value or `null` and never a wrapper.
   */
  const revision = useWorkspace((s) => revisionTabFor(s.boot, project, pane.id))
  const merge = useWorkspace((s) => mergeTabFor(s.boot, project, pane.id))
  const mergeTab = useWorkspace((s) => mergeTabIdFor(s.boot, project, pane.id))

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
    /*
     * An image is a file tab too, and the fork is here rather than in Rust. (M18)
     *
     * `imageKindFor` reads the extension and nothing else, which is the only input available
     * on a pane's first render — the alternative, a `PaneKind::Image` written by
     * `tab_open_file`, was rejected for two reasons. It would put a *derived* fact in
     * `workspace.json`, so a `.png` opened before the format list grew would come back as an
     * editor for ever; and it would have needed a second extension table in Rust, which is
     * precisely the drift `cide_core::image` refuses to carry (it sniffs bytes instead).
     *
     * Keying on `pane.kind === 'editor'` still, so this inherits the rule the comment below
     * states: splitting a File tab makes a *Claude* pane, and a tab-level fork would render a
     * second image viewer inside it.
     *
     * `ImagePane` is not lazy-loaded and must not become so. A pane that renders nothing for a
     * frame while a chunk arrives is a pane the layout measures at zero, which is the failure
     * `layout/paneHosts.ts` spends thirty lines preventing for terminals.
     */
    if (imageKindFor(editor.path) !== null) {
      return <ImagePane path={editor.path} root={cwd} onScreen={onScreen} />
    }
    return (
      <EditorPane
        path={editor.path}
        root={cwd}
        project={project}
        tab={editor.tab}
        onScreen={onScreen}
      />
    )
  }

  /*
   * A file as one commit left it — `TabKind::Revision`. (M18)
   *
   * Keyed on the **pane** kind as well as the tab, exactly like the file branch above and for
   * exactly its reason: `cmd::pane::default_intent` gives every non-console tab
   * `SplitIntent::NewClaude`, so splitting a revision tab produces a `PaneKind::Claude` pane
   * bound to a live conversation. Without the `pane.kind` test that pane would render a *second*
   * read-only buffer over the same blob instead of the terminal it is, and the agent's output
   * would go nowhere visible.
   *
   * Before the `held` splash, with the other document branches, and for the reason the revision
   * diff below states: a document must never be offered a Resume.
   *
   * `project` is required by the guard rather than defaulted, because every call this pane makes
   * — `gitLog.fileAt`, `git.repos`, `revisionFile.openTab`, `file.open` — takes a `ProjectId`.
   * A revision tab outside a project is unreachable (a `RepoId` only exists inside one), so this
   * is a type narrowing rather than a case with behaviour behind it.
   */
  /*
   * The conflict resolver. (M20)
   *
   * Guarded on the pane kind for `revision`'s reason, stated just below: splitting a non-console
   * tab produces a `PaneKind::Claude` pane, and without this test that pane would draw a second
   * resolver over the same conflict instead of the terminal it is.
   *
   * Before the `held` splash, with every other document branch, and for the reason the revision
   * diff states: a document must never be offered a Resume.
   */
  if (merge && mergeTab && pane.kind === 'editor' && project) {
    return <MergePane project={project} repo={merge.repo} path={merge.path} tab={mergeTab} />
  }

  if (revision && pane.kind === 'editor' && project) {
    return (
      <RevisionPane
        project={project}
        repo={revision.repo}
        path={revision.path}
        rev={revision.rev}
        from={revision.from}
        onScreen={onScreen}
      />
    )
  }

  /*
   * A revision diff — two frozen commits of one file — is a document too. (M18)
   *
   * Handled **here** rather than in `App.tsx`'s fork, which is where a working-tree git diff is
   * intercepted with the note *"a git diff replaces the pane rather than living in one"*. That
   * is the right treatment for the panel's diff, whose Stage footer and side switcher want the
   * whole tab. This one is read-only and has neither, so it is content like the Claude diff
   * below it and belongs in a pane — which also means a revision diff can be split beside a
   * terminal, and torn into its own window, without the tab-level fork having to learn about
   * detached panes.
   *
   * Before the `held` splash, with the other document branches, and that ordering is
   * load-bearing rather than tidy: `restore` is `undefined` for a diff pane (`lifecycle::
   * entry_for` plans an entry only for a Claude or Shell pane), but a restored workspace that
   * ever did plan one would otherwise offer to *resume* a document.
   */
  if (diff && diff.origin.kind === 'gitRevision' && project) {
    // `visible` is spread rather than passed, because `exactOptionalPropertyTypes` makes an
    // explicit `undefined` a different thing from an absent prop — and absent is what makes the
    // pane work the answer out of the workspace mirror instead of being told. See
    // `GitDiffPaneProps.visible`.
    return (
      <GitDiffPane
        project={project}
        spec={diff}
        {...(onScreen === undefined ? {} : { visible: onScreen })}
      />
    )
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
      roots={roots}
      onOpenPath={onOpenPath}
      onRevealPath={onRevealPath}
      onSessionBound={onSessionBound}
    />
  )
}
