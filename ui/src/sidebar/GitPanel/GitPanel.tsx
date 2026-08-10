/**
 * The 420px IDEA-style commit tool window.
 *
 * Composition only: tabs, toolbar, tree, guard bar, footer. Every piece of state and every
 * git call lives in `useGitPanel`, and every rule about rows and tri-state lives in
 * `model.ts`. That split is what lets the model be tested under node with no DOM
 * (`ui/scripts/check-git-tree.mjs`) and what keeps this file readable as a layout.
 *
 * The panel takes `project` and an optional place to put a diff, so the sidebar that hosts it
 * needs to know nothing about git:
 *
 * ```tsx
 * {view === 'git' && <GitPanel project={activeProjectId} />}
 * ```
 *
 * With `?git-story=<name>` on the URL it renders a fixture instead of talking to Rust —
 * `mock`, `guard`, `multi`, `empty`. See `fixture.ts` for why that exists.
 */
import { useEffect, useMemo, useState } from 'react'
import type { FileDiff, ProjectId, RepoId } from '@/ipc/client'
import { ChangesTree } from './ChangesTree'
import { CommitBox } from './CommitBox'
import { GuardBar } from './GuardBar'
import { ShelfList } from './ShelfList'
import { Toolbar } from './Toolbar'
import { allRepos, partialFiles, repoOf, summarize } from './model'
import { useGitPanel } from './useGitPanel'
import styles from './GitPanel.module.css'

type PanelTab = 'commit' | 'shelf'

export interface GitPanelProps {
  /** The active project, or `null` before one is open — then the panel is simply empty. */
  project: ProjectId | null
  /**
   * Where a double-clicked file's diff goes.
   *
   * Optional because the panel cannot open a pane by itself and must not pretend to: without
   * it the diff is still fetched and the panel says, in its one warning line, that there is
   * nowhere to show it. Wiring this is the host's job — see the README note in `index.ts`.
   */
  onOpenDiff?: ((diff: FileDiff, repo: RepoId) => void) | undefined
}

export function GitPanel({ project, onOpenDiff }: GitPanelProps) {
  const git = useGitPanel(project, { onOpenDiff })
  const [tab, setTab] = useState<PanelTab>('commit')

  const partial = useMemo(() => partialFiles(git.view), [git.view])
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
      <div className={styles.header}>
        <div className={styles.segmented} role="tablist" aria-label="Git">
          {(['commit', 'shelf'] as const).map((id) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              className={tab === id ? `${styles.segment} ${styles.segmentOn}` : styles.segment}
              onClick={() => setTab(id)}
            >
              {id === 'commit' ? 'Commit' : 'Shelf'}
            </button>
          ))}
        </div>
        {/* The branch belongs in the header of a commit window; with several repos it is
            per-repo and the tree's own rows carry it instead. */}
        {repos.length === 1 && repos[0] !== undefined && repos[0].branch.head !== '' && (
          <span className={styles.branch}>⑂ {repos[0].branch.head}</span>
        )}
      </div>

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
      />

      {git.story && (
        <p className={styles.note} data-audit="gitStoryNote">
          Fixture — no git commands are running.
        </p>
      )}
      {git.unavailable !== null && (
        <p className={`${styles.note} ${styles.noteWarn}`} data-audit="gitUnavailable">
          {git.unavailable}
        </p>
      )}
      {/*
        * A held partial selection changes what Commit writes, and nothing else in this panel
        * would show it: the row still reads as one ticked file. An invisible modifier on the
        * button that rewrites history is not acceptable, so it is named here with the way out
        * beside it. The count is files, not lines — the pane that made the selection is where
        * the lines are.
        */}
      {git.partials.length > 0 && (
        <p className={`${styles.note} ${styles.noteWarn}`} data-audit="gitPartials">
          {git.partials.length} file{git.partials.length === 1 ? '' : 's'} will be committed in
          part.{' '}
          <button type="button" className={styles.noteAction} onClick={git.clearPartials}>
            Use whole files
          </button>
        </p>
      )}

      <div className={styles.body} role="tabpanel" aria-label={tab === 'commit' ? 'Commit' : 'Shelf'}>
        {tab === 'commit' ? (
          <ChangesTree
            rows={git.rows}
            selected={git.selected}
            expanded={git.expanded}
            partial={partial}
            onToggleCheck={git.toggleCheck}
            onToggleExpand={git.toggleExpand}
            onOpenDiff={git.openDiff}
          />
        ) : (
          <ShelfList
            entries={git.shelf}
            showRepo={repos.length > 1}
            labelFor={labelFor}
            onUnshelve={git.unshelve}
          />
        )}
      </div>

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
          summary={summary}
          canAmend={canAmend}
          canCommit={hasSelection && git.busy === null}
          busy={git.busy}
          onMessage={git.setMessage}
          onAmend={git.setAmend}
          onCommit={() => git.commit(false)}
          onCommitAndPush={() => git.commit(true)}
        />
      )}
    </aside>
  )
}
