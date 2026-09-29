/**
 * Which working tree a git surface shows: the project's own checkout, or one of the agent
 * checkouts under `.cide/worktrees/`. Drawn in the Git panel's header and the Log's filter bar.
 *
 * # The trigger names a branch, not a directory
 *
 * The Git panel's header used to read the branch the project had checked out, and this replaces
 * that readout — so the closed control still says a branch: the project's, or the one the chosen
 * checkout is on (`cide/<role>-<task>`, read from its `HEAD` rather than assumed). The checkout's
 * directory name is the row's `detail`, which is how two checkouts on one branch stay apart.
 *
 * # Server-rendered
 *
 * Both hosts are rendered under node by their checks (`check:render`, `check:log-render`), so
 * this imports the kit's `Select` and types and nothing else. `Select` touches `window` and
 * `document` only inside effects and while open, neither of which a server render reaches. The
 * list is loaded by `useWorktrees`, one level up, and arrives as a prop.
 */
import type { WorktreeInfo } from '@/ipc/generated'
import { Select, type SelectOption } from '@/kit/components/Select'

/** The project's own checkout, as a `Select` value. No worktree path is empty. */
export const PROJECT_CHECKOUT = ''

export interface WorktreeSelectProps {
  /** The agent checkouts, as `git.worktrees` listed them. */
  worktrees: readonly WorktreeInfo[]
  /** The chosen checkout's path (`WorktreeInfo.repo.root`), or `null` for the project's. */
  value: string | null
  /** The branch the project has checked out, for the first row; `null` when unknown. */
  projectBranch: string | null
  onChange: (worktree: string | null) => void
  /** The `data-audit` name the host's check finds this by. */
  audit: string
}

export function worktreeOptions(
  worktrees: readonly WorktreeInfo[],
  projectBranch: string | null,
): SelectOption[] {
  return [
    {
      value: PROJECT_CHECKOUT,
      label: projectBranch !== null && projectBranch !== '' ? projectBranch : 'Project',
      icon: 'git-branch',
      detail: 'project',
    },
    ...worktrees.map((wt) => ({
      value: wt.repo.root,
      label: wt.branch,
      icon: 'git-branch' as const,
      detail: wt.repo.name,
    })),
  ]
}

export function WorktreeSelect({
  worktrees,
  value,
  projectBranch,
  onChange,
  audit,
}: WorktreeSelectProps) {
  return (
    <span data-audit={audit} data-value={value ?? PROJECT_CHECKOUT}>
      <Select
        size="sm"
        aria-label="Worktree"
        value={value ?? PROJECT_CHECKOUT}
        options={worktreeOptions(worktrees, projectBranch)}
        onChange={(next) => onChange(next === PROJECT_CHECKOUT ? null : next)}
      />
    </span>
  )
}
