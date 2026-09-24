/**
 * The 420px IDEA-style commit tool window — everything except its connection to the window.
 *
 * Composition only: tabs, toolbar, tree, guard bar, footer. Every piece of state and every
 * git call lives in `useGitPanel`, and every rule about rows and tri-state lives in
 * `model.ts`. That split is what lets the model be tested under node with no DOM
 * (`ui/scripts/check-git-tree.mjs`) and what keeps this file readable as a layout.
 *
 * # Why this is a *view* and `GitPanelHost` is the panel
 *
 * `ui/scripts/check-git-render.mjs` renders this file under node with `react-dom/server` —
 * that check is the only reason we know the panel paints rather than merely compiles, and it
 * caught the bug where every repository was silently dropped. It renders with `window` and
 * `location` stubbed and **nothing else**, deliberately: a check that fakes a browser proves
 * things about the fake.
 *
 * Two things this panel now needs are unreachable under that rule. `useContextMenu` reads the
 * window's keymap from `@/store/workspace`, and `useIconTheme` reads the theme from the same
 * store — and that store imports `layout/paneHosts`, which calls `document.createElement` at
 * module scope and pulls the xterm addons in behind it. Importing either from here turns the
 * render check into `ReferenceError: document is not defined`.
 *
 * So the two window-shaped concerns are lifted one level, into `GitPanelHost.tsx`, and arrive
 * here as ordinary props: a theme string and a pair of menu handles. This file keeps its
 * import graph free of the DOM, the check keeps rendering the thing it asserts about, and the
 * menu is wired in exactly one place. `index.ts` exports the host as `GitPanel`, so nothing
 * outside this directory notices.
 *
 * With `?git-story=<name>` on the URL it renders a fixture instead of talking to Rust —
 * `mock`, `guard`, `multi`, `empty`. See `fixture.ts` for why that exists.
 */
import { Button } from '@/kit/components/Button'
import { Banner } from '@/kit/components/Feedback'
import { PanelHeader, Tabs } from '@/kit/components/Surface'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import type { ProjectId, RepoId } from '@/ipc/client'
import type { IconTheme } from '@/icons/iconFor'
import { ChangelistDialog } from './ChangelistDialog'
import { ChangesTree } from './ChangesTree'
import { CommitBox } from './CommitBox'
import { ConfirmDestructive } from '@/chrome/ConfirmDestructive'
import { useFocusRequested } from '@/chrome/focusRequests'
import { GuardBar } from './GuardBar'
import { MergeBar } from './MergeBar'
import { ShelfList } from './ShelfList'
import { Toolbar } from './Toolbar'
import { allRepos, canCommit, repoOf, summarize } from './model'
import { useGitDiffTabOpen } from './openDiffTabs'
import type { GitPanelActions, GitPanelModel } from './useGitPanel'
import { Icon } from '@/icons/Icon'

import styles from './GitPanel.module.css'

type PanelTab = 'commit' | 'shelf'

/** The two halves of a `useContextMenu` handle, threaded down from the host. */
export interface TreeMenu {
  onContextMenu: (e: React.MouseEvent) => void
  /** Must be rendered somewhere; it portals. */
  menu: ReactNode
}

export interface GitPanelViewProps {
  /** The active project, or `null` before one is open — then the panel is simply empty. */
  project: ProjectId | null
  /** The model, built by the host so that the host's menu can act on the same one. */
  git: GitPanelModel & GitPanelActions
  /**
   * `dark` or `light`, for the row icons.
   *
   * A prop rather than a `useIconTheme()` call: that hook reads the workspace store, which
   * this module may not import. See the header.
   */
  iconTheme: IconTheme
  /** Absent in a render with no window — the SSR check, and any fixture harness. */
  treeMenu?: TreeMenu | undefined
  /**
   * *Update project* — the toolbar's third glyph, which had been disabled since it was written.
   *
   * Optional for `onOpenDiff`'s reason: this panel cannot run a command by itself and must not
   * pretend to. It is routed through `runCommand('git.pull')` by `App.tsx` rather than calling
   * `branchApi.pull` here — the `file.reveal` precedent, one handler and one copy of the
   * preconditions for every surface — so the strategy dialog, the retry and the aggregated
   * notice are one code path and not two.
   */
  onUpdate?: (() => void) | undefined
}

export function GitPanelView({
  project,
  git,
  iconTheme,
  treeMenu,
  onUpdate,
}: GitPanelViewProps) {
  const [tab, setTab] = useState<PanelTab>('commit')
  /*
   * The cursor used to be a `useState` right here, held above `ChangesTree` because that
   * component unmounts every time the Shelf tab is opened and a cursor that resets on a glance
   * at the shelf is a cursor nobody can rely on. It has moved one level further down, into
   * `useGitPanel`, and it took the row selection with it: the context menu builds its scope in
   * `GitPanelHost` — *above* this file — so anything the menu and the drag must agree on has
   * to be visible from the model. The Shelf-tab argument is unchanged and still holds there.
   */
  const diffOpen = useGitDiffTabOpen(project)

  /*
   * *Commit changes…* asked for the message box, and the Shelf tab is showing.
   *
   * `CommitBox` is mounted only under the Commit tab, so without this the command would reveal
   * the panel, park a focus request nobody could answer, and look exactly like a palette row
   * that does nothing — for a user who happened to leave the panel on Shelf. The tab is local
   * state here, so this is the only place that can move it.
   *
   * The request itself is *not* cleared here: `CommitBox` consumes it, one render later, once
   * the tab change has mounted it. Clearing it here would answer the request by throwing it
   * away, which is the same bug with an extra step.
   */
  const commitFocusWanted = useFocusRequested('commitMessage')
  useEffect(() => {
    if (commitFocusWanted) setTab('commit')
  }, [commitFocusWanted])

  const summary = useMemo(() => summarize(git.picked), [git.picked])
  const hasSelection = git.picked.length > 0
  const repos = useMemo(() => allRepos(git.view), [git.view])
  // Amend needs a commit to amend. On an unborn branch there is none, and the checkbox is
  // disabled rather than removed so its absence is explicable.
  const canAmend = repos.some((r) => !r.branch.unborn)

  // The Shelf tab reads a different command from the Commit tab, so it is loaded when it is
  // opened rather than kept warm — `git_shelf_list` is one round trip per repository and the
  // tab is the only thing that reads it.
  const { refreshShelf } = git
  useEffect(() => {
    if (tab === 'shelf') refreshShelf()
  }, [tab, refreshShelf])

  const labelFor = (repo: RepoId) => repoOf(git.view, repo)?.name ?? repo

  return (
    <aside className={styles.panel} data-audit="gitPanel" aria-label="Commit">
      {/* The kit's panel frame: the header every sidebar panel tops with, then `Tabs` — Commit
          and Shelf switch *what* is shown, which is tabs and not a segmented filter. The branch
          belongs in the header of a commit window; with several repos it is per-repo and the
          tree's own rows carry it instead. */}
      <PanelHeader
        title="Git"
        tools={
          repos.length === 1 && repos[0] !== undefined && repos[0].branch.head !== '' ? (
            <span className={styles.branch}>
              <Icon name="git-branch" size={0} />
              <span className={styles.branchName}>{repos[0].branch.head}</span>
            </span>
          ) : undefined
        }
      />
      <Tabs
        label="Git"
        value={tab}
        onChange={setTab}
        tabs={[
          { value: 'commit', label: 'Commit' },
          { value: 'shelf', label: 'Shelf' },
        ]}
      />

      <Toolbar
        stagingArea={git.stagingArea}
        hasSelection={hasSelection}
        onRefresh={git.refresh}
        onUnstage={git.unstage}
        onShelve={git.shelve}
        onShowDiff={git.showSelectedDiff}
        onStagingArea={git.setStagingArea}
        onCollapseAll={() => git.setAllExpanded(false)}
        onExpandAll={() => git.setAllExpanded(true)}
        // Optional all the way down, and still optional here: a window with no dispatcher — the
        // fixture stories, and `check:render`'s SSR pass — must draw the button disabled with a
        // reason rather than a control wired to nothing.
        {...(onUpdate !== undefined ? { onUpdate } : {})}
        {...(repos.length === 1
          ? { onNewChangelist: () => git.newChangelist() }
          : // With several repositories the toolbar cannot know which one a new changelist
            // belongs to, and a changelist lives in exactly one repo's sidecar. The button
            // disables itself and says where the gesture does know — a repository row's menu
            // — rather than guessing the first root.
            {})}
        repoCount={repos.length}
      />

      {git.story && (
        <div data-audit="gitStoryNote">
          <Banner>Fixture — no git commands are running.</Banner>
        </div>
      )}
      {git.unavailable !== null && (
        /*
         * `title` because `.note` is one ellipsized line and these sentences are long: a git
         * refusal names the path, says what refused, and says what to do instead, and in a
         * narrow sidebar the third part is exactly what falls off the end. The tooltip is the
         * only way to read the rest — the note cannot wrap without pushing the tree down on
         * every transient failure.
         */
        <div data-audit="gitUnavailable" title={git.unavailable}>
          <Banner tone="warn">{git.unavailable}</Banner>
        </div>
      )}
      {/*
        * A held partial selection changes what Commit writes, and nothing else in this panel
        * would show it: the row still reads as one ticked file. An invisible modifier on the
        * button that rewrites history is not acceptable, so it is named here with the way out
        * beside it. The count is files, not lines — the pane that made the selection is where
        * the lines are.
        */}
      {git.partials.length > 0 && (
        <div data-audit="gitPartials">
          <Banner
            tone="warn"
            action={
              <Button size="sm" variant="link" onClick={git.clearPartials}>
                Use whole files
              </Button>
            }
          >
            {git.partials.length} file{git.partials.length === 1 ? '' : 's'} will be committed in
            part.
          </Banner>
        </div>
      )}

      <div className={styles.body} role="tabpanel" aria-label={tab === 'commit' ? 'Commit' : 'Shelf'}>
        {tab === 'commit' ? (
          <ChangesTree
            rows={git.rows}
            view={git.view}
            selected={git.selected}
            selection={git.selection}
            carried={git.carried}
            expanded={git.expanded}
            current={git.current}
            onCurrent={git.setCurrent}
            onPress={git.pressRow}
            onRelease={git.releaseRow}
            onKeyTo={git.keyToRow}
            onSelectAll={git.selectAllRows}
            onCollapseSelection={git.collapseSelection}
            diffOpen={diffOpen}
            iconTheme={iconTheme}
            onToggleCheck={git.toggleCheck}
            onToggleCheckSelected={git.toggleCheckSelected}
            onToggleExpand={git.toggleExpand}
            onOpenDiff={git.openDiff}
            onOpenFile={git.openFile}
            /* Drag and drop's only connection to git. The rules live in `dragDrop.ts` and the
               gesture in `useChangesDrag.ts`; by the time this is called the target has been
               named and the same-list paths dropped.

               Two of them, because a drop out of `Unversioned Files` is a different operation:
               it adds the paths to git before filing them. Passing only the first is what the
               panel did until the user reported that unversioned files could not be dragged
               anywhere at all. */
            onMovePaths={git.movePaths}
            onTrackPaths={git.trackPaths}
            onContextMenu={treeMenu?.onContextMenu}
            menu={treeMenu?.menu}
          />
        ) : (
          <ShelfList
            entries={git.shelf}
            showRepo={repos.length > 1}
            labelFor={labelFor}
            onUnshelve={git.unshelve}
            onUnshelveKeep={git.unshelveKeep}
            onDrop={git.dropShelf}
          />
        )}
      </div>

      {/*
        * One bar per repository mid-merge, above the staging guard. (M20)
        *
        * Rendered for both tabs, for `GuardBar`'s reason one step stronger: a merge in progress
        * is not merely still true while the Shelf tab is showing — it *blocks the commit*, and a
        * user who cannot see why Commit refuses will conclude the button is broken.
        *
        * Above the guard bar because it is the outer fact: while a merge is in flight the index
        * is the merge's, so "staging changed outside cide" is both expected and not the thing to
        * act on first.
        */}
      {Object.entries(git.merges).map(([repo, state]) => (
        <MergeBar
          key={repo}
          state={state}
          busy={git.busy !== null}
          onContinue={() => git.continueMerge(repo)}
          onAbort={() => git.abortMerge(repo)}
          onResolveSimple={() => git.resolveSimpleConflicts(repo)}
          onShowList={() => git.showConflictList(repo)}
        />
      ))}

      {/* Rendered for both tabs: an index that moved under us is still true while the
          Shelf tab is showing, and hiding the warning behind a tab is how it gets missed. */}
      <GuardBar
        repos={git.diverged}
        showRepo={repos.length > 1}
        labelFor={labelFor}
        onReload={git.reloadIndex}
        onOverwrite={git.overwriteIndex}
      />

      {tab === 'commit' && (
        <CommitBox
          message={git.message}
          amend={git.amend}
          /* The log's abbreviation, straight through — `null` for the checkbox's own meaning.
             See `CommitBoxProps.amendOf` for why it is on the label rather than hidden. */
          amendOf={git.amendOf?.shortOid ?? null}
          summary={summary}
          canAmend={canAmend}
/* The rule is in `model.ts` so `check:git` can compile and run it — including
             the reword case, which is the one place Commit is live on an empty selection. */
          canCommit={canCommit({
            picked: git.picked.length,
            reword: git.rewordRepo !== null,
            busy: git.busy,
          })}
          busy={git.busy}
          onMessage={git.setMessage}
          onAmend={git.setAmend}
          onCommit={() => git.commit(false)}
          onCommitAndPush={() => git.commit(true)}
        />
      )}

      {/*
        * Both overlays are mounted by the panel rather than by `App.tsx`, and the conclusion
        * still holds — every gesture in this feature is reachable with no line in a file this
        * session does not own; see the note at the foot of `GitPanelHost`. The reason changed
        * in M19: `OverlayCard` now portals its scrim to `document.body` rather than drawing it
        * in place, because a card rendered inside a tab panel was capped by that panel's own
        * stacking context (`TabContent.module.css` gives it `z-index: 0/1`) and could not rise
        * above the sidebar splitter beside it. Mounting here is still right; it is simply no
        * longer the thing that puts the scrim over the window.
        */}
      {git.dialog !== null && (
        <ChangelistDialog
          state={git.dialog}
          onCancel={git.dismissDialog}
          onPick={git.pickChangelist}
          onSubmitName={git.submitChangelistName}
        />
      )}
      {git.confirm !== null && (
        <ConfirmDestructive
          state={git.confirm}
          onCancel={git.dismissConfirm}
          onConfirm={git.runConfirm}
        />
      )}
    </aside>
  )
}
