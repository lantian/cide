/**
 * The 420px IDEA-style commit tool window.
 *
 * Composition only: tabs, toolbar, tree, guard bar, footer. Every piece of state and every
 * git call lives in `useGitPanel`, and every rule about rows and tri-state lives in
 * `model.ts`. That split is what lets the model be tested under node with no DOM
 * (`ui/scripts/check-git-tree.mjs`) and what keeps this file readable as a layout.
 *
 * The panel takes `project` and nothing else, so the sidebar that hosts it needs to know
 * nothing about git. Mount it as:
 *
 * ```tsx
 * {view === 'git' && <GitPanel project={activeProjectId} />}
 * ```
 *
 * With `?git-story=<name>` on the URL it renders a fixture instead of talking to Rust —
 * `mock`, `guard`, `multi`, `empty`. See `fixture.ts` for why that exists.
 */
import { useMemo, useState } from 'react'
import type { ProjectId } from '@/ipc/client'
import { ChangesTree } from './ChangesTree'
import { CommitBox } from './CommitBox'
import { GuardBar } from './GuardBar'
import { ShelfList } from './ShelfList'
import { Toolbar } from './Toolbar'
import { partialFiles, summarize } from './model'
import { useGitPanel } from './useGitPanel'
import styles from './GitPanel.module.css'

type PanelTab = 'commit' | 'shelf'

export interface GitPanelProps {
  /** The active project, or `null` before one is open — then the panel is simply empty. */
  project: ProjectId | null
}

export function GitPanel({ project }: GitPanelProps) {
  const git = useGitPanel(project)
  const [tab, setTab] = useState<PanelTab>('commit')

  const partial = useMemo(() => partialFiles(git.tree), [git.tree])
  const summary = useMemo(() => summarize(git.picked), [git.picked])
  const hasSelection = git.picked.length > 0
  // Amend needs a commit to amend. On an unborn branch every repo reports no head message,
  // and the checkbox is disabled rather than removed so its absence is explicable.
  const canAmend = git.tree.repos.some((r) => r.headMessage !== undefined)

  const labelFor = (root: string) =>
    git.tree.repos.find((r) => r.root === root)?.label ?? (root.split('/').pop() ?? root)

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
        {git.tree.repos.length === 1 && git.tree.repos[0]?.branch !== undefined && (
          <span className={styles.branch}>⑂ {git.tree.repos[0].branch}</span>
        )}
      </div>

      <Toolbar
        stagingArea={git.stagingArea}
        hasSelection={hasSelection}
        onRefresh={git.refresh}
        onUnstage={git.unstage}
        onShelve={git.shelve}
        onShowDiff={() => {
          const first = git.rows.find((r) => r.kind === 'file' && git.selected.has(r.id))
          if (first) git.openDiff(first)
        }}
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
          <ShelfList entries={git.shelf} onUnshelve={() => git.refresh()} />
        )}
      </div>

      {/* Rendered for both tabs: an index that moved under us is still true while the
          Shelf tab is showing, and hiding the warning behind a tab is how it gets missed. */}
      <GuardBar
        repos={git.diverged}
        showRepo={git.tree.repos.length > 1}
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
