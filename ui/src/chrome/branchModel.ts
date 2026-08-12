/**
 * Every decision the branch selector makes, with no DOM and no React in it.
 *
 * The popup itself is a list, a text box and some buttons; what is worth getting right — and
 * what is worth being able to test in a harness that has no browser — is *which* rows appear,
 * *which* actions each row may offer, and *what a refusal says*. Those live here.
 *
 * `ui/scripts/check-branches.mjs` compiles this file on its own and drives the functions
 * directly, the same arrangement `chrome/menuModel.ts` and `sidebar/GitPanel/model.ts` use.
 * That only works while this module stays free of runtime imports: **type-only imports from
 * `@/ipc/generated`, nothing else.**
 *
 * # What the selector deliberately does not offer
 *
 * IDEA's branch popup has *compare with current*, *merge into current*, *rebase current onto*
 * and *delete on remote*. None of them are here, and their absence is a decision rather than a
 * backlog:
 *
 * * **Merge and rebase** end in conflicts often enough that they are only usable with a
 *   conflict-resolution surface. cide's diff panes are read-only against HEAD, so a merge that
 *   conflicts would leave a working tree nothing in the app can finish — and `git pull` here
 *   is fast-forward-only for exactly the same reason.
 * * **Compare** needs a branch-to-branch diff view. The diff tabs compare a file against HEAD.
 * * **Delete on remote** is a push of an empty refspec. It is not undoable by the person who
 *   ran it and it must not share a menu item with deleting a local ref.
 *
 * A menu entry that reports "not implemented" is worse than an absent one: it costs the user a
 * click and a mode switch to learn nothing. When one of these lands it arrives with its
 * surface, not before it.
 */
import type { BranchInfo, BranchList, BranchRef, RepoId } from '@/ipc/generated'

// --- the status bar's one slot ------------------------------------------------------------

/** What the widget shows when the project has no repository at all. */
export const NO_REPO = 'no repo'

/** What it shows before the first `git_branch_list` has answered. */
export const LOADING = '…'

/**
 * The label in the status bar: `main`, `main ↑2↓1`, or `a1b2c3d4 detached`.
 *
 * The counts are only drawn when non-zero. A permanent `↑0↓0` is four characters of noise in
 * a 24px bar and trains the eye to stop reading the slot, which is the one thing a status
 * readout cannot afford — the same argument `StatusBar` makes for degrading its diagnostics
 * group to `—` rather than printing a confident zero.
 */
export function headLabel(head: BranchInfo | null): string {
  if (head === null) return NO_REPO
  if (head.detached) return `${head.head} detached`
  const ahead = head.ahead > 0 ? ` ↑${head.ahead}` : ''
  const behind = head.behind > 0 ? ` ↓${head.behind}` : ''
  return `${head.head}${ahead}${behind}`
}

/**
 * The tooltip. Says the things the label had to drop, and says them in words.
 *
 * `unborn` gets a sentence of its own because "on main with no commits" and "on main" look
 * identical in the bar and behave very differently — half the popup's actions need a commit
 * to point at.
 */
export function headTitle(head: BranchInfo | null, repos: number): string {
  if (head === null) return 'No git repository under this project’s roots'
  const parts: string[] = []
  parts.push(head.detached ? `Detached at ${head.head}` : `On branch ${head.head}`)
  if (head.unborn) parts.push('no commits yet')
  if (head.upstream !== null) {
    parts.push(`tracking ${head.upstream}`)
    if (head.ahead > 0 || head.behind > 0) {
      parts.push(`${head.ahead} ahead, ${head.behind} behind`)
    }
  } else if (!head.detached) {
    parts.push('no upstream')
  }
  if (head.operation !== null) parts.push(`${head.operation} in progress`)
  if (repos > 1) parts.push(`${repos} repositories in this project`)
  return parts.join(' · ')
}

/**
 * The repository the widget speaks for, or `null`.
 *
 * The first in root order — which is the project's first root, the one the user opened. With
 * several repositories the widget names one and the popup lets you pick; picking *for* the
 * user is unavoidable in a slot this size, and the first root is the only choice that is
 * stable across restarts.
 */
export function primaryRepo(lists: readonly BranchList[]): BranchList | null {
  return lists[0] ?? null
}

/** The list for `repo`, falling back to the primary one when that id is gone. */
export function listFor(lists: readonly BranchList[], repo: RepoId | null): BranchList | null {
  if (repo === null) return primaryRepo(lists)
  return lists.find((entry) => entry.repo.id === repo) ?? primaryRepo(lists)
}

// --- the rows -----------------------------------------------------------------------------

/**
 * The rows the popup draws, in order, filtered by `query`.
 *
 * **Current branch first, then locals, then remote-tracking.** Rust already sorted each side
 * by recency (see `cide_git::branch::collect`); pinning the current one is a display decision
 * and it is here so it can be asserted without a repository. It matters because the current
 * branch is the row every other action is relative to — *new branch from here* and *delete*
 * both mean something different depending on where you are standing.
 *
 * Matching is a case-insensitive substring of the name. Not fuzzy: `mn` finding `main` is
 * useful in a file picker over ten thousand paths and actively confusing over forty branch
 * names, where it also matches `maintenance`, `mono-nightly` and `demand`. Substring is what
 * IDEA's own branch filter does.
 */
export function visibleBranches(list: BranchList | null, query: string): BranchRef[] {
  if (list === null) return []
  const ordered = [...list.local, ...list.remote]
  const current = ordered.filter((entry) => entry.current)
  const rest = ordered.filter((entry) => !entry.current)
  return [...current, ...rest].filter((entry) => matches(entry, query))
}

/** Whether one row survives the filter. An empty or blank query keeps everything. */
export function matches(entry: BranchRef, query: string): boolean {
  const needle = query.trim().toLowerCase()
  return needle === '' || entry.name.toLowerCase().includes(needle)
}

/** `origin/feature/login` → `origin`, and `null` for a local branch. */
export function remoteOf(entry: BranchRef): string | null {
  if (!entry.remote) return null
  const cut = entry.name.indexOf('/')
  return cut < 0 ? null : entry.name.slice(0, cut)
}

// --- what a row may do ---------------------------------------------------------------------

/** The per-branch actions, in the order the row menu lists them. */
export type BranchAction = 'checkout' | 'newFrom' | 'rename' | 'delete'

/**
 * Which actions `entry` offers.
 *
 * Deriving the list rather than rendering four buttons and disabling three is the same rule
 * the rest of this app follows for menus: an item that is present and dead is a claim that
 * something is possible here. The two exclusions are load-bearing:
 *
 * * the **current** branch cannot be checked out again (nothing would happen) or deleted
 *   (git refuses, and so does `cide_git::branch::delete`);
 * * a **remote-tracking** ref cannot be renamed or deleted, because it is a cache of the
 *   remote's names — the next fetch would put it straight back. *Checkout* on one is offered
 *   and means "create a local branch that tracks it", which is what git and IDEA both do.
 */
export function actionsFor(entry: BranchRef): BranchAction[] {
  const actions: BranchAction[] = []
  if (!entry.current) actions.push('checkout')
  actions.push('newFrom')
  if (!entry.remote) {
    actions.push('rename')
    if (!entry.current) actions.push('delete')
  }
  return actions
}

/**
 * Whether the popup's *Pull* is offered for this repository.
 *
 * Needs an upstream to pull from, a commit to move, and no half-finished operation. Offering
 * it without an upstream would produce `noUpstream` on every click, which is a control that
 * only ever fails.
 */
export function canPull(head: BranchInfo | null): boolean {
  return head !== null && !head.unborn && !head.detached && head.upstream !== null
}

/** Whether *Push* is offered: any born, non-detached branch — with or without an upstream. */
export function canPush(head: BranchInfo | null): boolean {
  return head !== null && !head.unborn && !head.detached
}

// --- refusals ------------------------------------------------------------------------------

/** A wire error, as far as this module needs to look at one. */
interface WireError {
  kind: string
  detail?: unknown
}

function wire(error: unknown): WireError | null {
  if (typeof error !== 'object' || error === null) return null
  const kind = (error as { kind?: unknown }).kind
  if (typeof kind !== 'string') return null
  return { kind, detail: (error as { detail?: unknown }).detail }
}

function field(error: WireError, name: string): unknown {
  const detail = error.detail
  if (typeof detail !== 'object' || detail === null) return undefined
  return (detail as Record<string, unknown>)[name]
}

/**
 * The tag of a wire error, or `null` for anything that is not one.
 *
 * A guarded read rather than `(error as {kind?: string}).kind`, which throws on `null` — and
 * `null` is exactly what a rejection can be. One catch block that itself threw would turn a
 * recoverable refusal into an unhandled rejection.
 */
export function kindOf(error: unknown): string | null {
  return wire(error)?.kind ?? null
}

/**
 * The checkout refusal, unpacked — or `null` when the failure was something else.
 *
 * This is the one error the popup does not merely print. `paths` is the answer the user needs,
 * and the two stash offers are only correct for this error: offering *stash and switch* after
 * a `notFastForward` would stash the work and then fail again for the same reason.
 */
export interface Refusal {
  branch: string
  paths: string[]
}

export function refusalOf(error: unknown): Refusal | null {
  const parsed = wire(error)
  if (parsed === null || parsed.kind !== 'checkoutWouldOverwrite') return null
  const branch = field(parsed, 'branch')
  const paths = field(parsed, 'paths')
  if (typeof branch !== 'string' || !Array.isArray(paths)) return null
  return { branch, paths: paths.filter((p): p is string => typeof p === 'string') }
}

/**
 * A `GitError` as a sentence, for everything the popup does not have a dedicated panel for.
 *
 * Written out per variant rather than falling back to `String(error)` because the wire form
 * is a tagged object: `String({kind: 'noUpstream', …})` is `[object Object]`, which is how a
 * control ends up appearing to do nothing at all. Anything genuinely unrecognised is still
 * shown — with its tag — rather than swallowed.
 */
export function explain(error: unknown): string {
  const parsed = wire(error)
  if (parsed === null) return error instanceof Error ? error.message : String(error)

  const name = (key: string): string => {
    const value = field(parsed, key)
    return typeof value === 'string' ? value : '?'
  }
  const count = (key: string): number => {
    const value = field(parsed, key)
    return typeof value === 'number' ? value : 0
  }

  switch (parsed.kind) {
    case 'checkoutWouldOverwrite': {
      const refusal = refusalOf(error)
      const paths = refusal === null ? '' : `: ${refusal.paths.join(', ')}`
      return `Switching to ${name('branch')} would overwrite local changes${paths}`
    }
    case 'branchExists':
      return `A branch named ${name('name')} already exists`
    case 'invalidBranchName':
      return `${name('name') === '' ? 'A branch name' : name('name')} is not a valid branch name — no spaces, no “..”, no trailing “.lock”`
    case 'branchNotMerged':
      return `${name('name')} has commits that are on no other branch. Deleting it loses them.`
    case 'branchIsCurrent':
      return `${name('name')} is the branch you are on. Switch somewhere else first.`
    case 'noSuchBranch':
      return `${name('name')} is gone — someone deleted it since this list was drawn`
    case 'notFastForward':
      return `${name('branch')} is ${count('ahead')} ahead and ${count('behind')} behind its upstream. cide only fast-forwards; merge or rebase in a terminal.`
    case 'noUpstream':
      return `${name('branch')} has no upstream branch to pull from`
    case 'operationInProgress':
      return `A ${name('operation')} is in progress. Finish or abort it first.`
    case 'unborn':
      return 'This branch has no commits yet'
    case 'conflicted':
      return 'The working tree has unresolved conflicts'
    case 'fetch':
      return `Fetch failed: ${name('output').trim()}`
    case 'push':
      return `Push failed: ${name('output').trim()}`
    case 'notARepository':
      return `${name('path')} is not inside a git repository`
    case 'io':
    case 'git':
      return name('detail')
    default:
      return `git: ${parsed.kind}`
  }
}

/**
 * What a completed checkout has to say, or `''` when it has nothing worth a line.
 *
 * The silent case is the common one — a clean switch needs no announcement, the branch name in
 * the status bar changed and that is the feedback. The three noisy cases are the ones where
 * something happened that the user did not directly ask for.
 */
export function checkoutNote(outcome: {
  branch: string
  createdFromRemote: string | null
  stashed: string | null
  restoreFailed: string | null
}): string {
  if (outcome.restoreFailed !== null) return outcome.restoreFailed
  if (outcome.stashed !== null) {
    return `Switched to ${outcome.branch}. Your changes are in the stash — “${outcome.stashed}”.`
  }
  if (outcome.createdFromRemote !== null) {
    return `Created ${outcome.branch} tracking ${outcome.createdFromRemote}`
  }
  return ''
}

/** What a fetch or a fast-forward pull has to say. Never empty: the user asked for network. */
export function fetchNote(outcome: { remote: string; advanced: number; output: string }): string {
  if (outcome.advanced > 0) {
    const commits = outcome.advanced === 1 ? '1 commit' : `${outcome.advanced} commits`
    return `Fast-forwarded ${commits} from ${outcome.remote}`
  }
  const said = outcome.output.trim()
  return said === '' ? `Already up to date with ${outcome.remote}` : said
}

/*
 * Runtime values only, no imports: `check-branches.mjs` compiles this file alone and loads the
 * emitted `.js` in node. Adding a value import — a store, an icon, `@/ipc/client` — breaks
 * that, and the check would start failing with a module-resolution error rather than telling
 * anyone why. The same note is at the foot of `sidebar/GitPanel/types.ts`.
 */
