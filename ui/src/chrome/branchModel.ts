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

// --- where the keyboard is -----------------------------------------------------------------

/**
 * Where the popup's keyboard highlight sits: in the filter field, or on one visible row.
 *
 * > *"when this popup with list of branches is opening it focus on filter input - this is ok,
 * > but when i pressing UP/DOWN buttons - i should be able to travers through branches and back
 * > to filter input if top of the list of branches"*
 *
 * The filter is a *place* in this model, not the absence of a selection, and that is the whole
 * point of the union: `-1 means the field` inside a plain number would have made the one
 * transition the user actually asked for — Up from the first branch landing back in the filter
 * — an off-by-one waiting to be clamped away by somebody tidying up.
 *
 * DOM focus never leaves the input. The rows are real `<button>`s and moving focus onto them
 * would have been the obvious implementation, but it loses twice: the current branch's button
 * is `disabled` (it cannot be checked out again), and a disabled button cannot take focus — so
 * the branch you are standing on would be the one row the arrows could never reach — and a row
 * with focus swallows typing, so the user could no longer narrow the list without a trip back
 * to the field. Highlight-in-the-model, focus-in-the-field is what `overlays/CommandPalette.tsx`
 * and `overlays/ScratchType.tsx` already do; this is the same arrangement with one extra state.
 */
export type BranchFocus = { kind: 'filter' } | { kind: 'row'; index: number }

/** The resting place: the filter field, which is what the popup opens on. */
export const FILTER: BranchFocus = { kind: 'filter' }

/**
 * How far Page Up / Page Down jump.
 *
 * Ten, the same as `overlays/listKeys.ts::PAGE_ROWS`, and deliberately *not* imported from it:
 * this module compiles standalone under `check-branches.mjs` with no module resolution at all
 * (see the foot-note), and a value import would break that with a resolution error rather than
 * a sentence. The number is duplicated; the reasoning is not, and it is one number.
 */
export const PAGE_ROWS = 10

/**
 * The keystroke, resolved against a list of `count` rows with `focus` highlighted.
 *
 * `null` means *nothing to do with the list* — either the key is not one of ours, or it is but
 * there is nowhere for it to go. The caller uses that to decide whether to `preventDefault`, so
 * a Down on the last row leaves the text field's own caret behaviour alone rather than eating
 * the key.
 *
 * **Movement does not wrap**, unlike the file picker's. Wrapping would make Up from the first
 * branch select the *last* one, and Up from the first branch is precisely the gesture that has
 * to return to the filter field. The two rules cannot both hold, and the user asked for this
 * one; a bottom row that does not roll round to the top is the smaller loss, and End is here.
 *
 * `Home` and `End` do nothing while the focus is in the field, because there they are the
 * caret's keys and a filter you cannot jump to the start of is a filter you cannot edit.
 * `Escape` is absent for a different reason: the popup's scrim already owns it, and it means
 * *back out one panel*, which is a question about `Mode` that this reducer cannot see.
 */
export function navigate(key: string, focus: BranchFocus, count: number): BranchFocus | null {
  // An empty list — no branches, or a filter that matched nothing — has no row to move onto,
  // and the highlight is already in the field. Every key is the field's.
  if (count === 0) return null

  // `-1` for the field *inside* this function only: the arithmetic below is "one place up or
  // down a column that starts with the filter", and writing it out as branches per key made
  // four copies of the same clamp. The union is what crosses the boundary.
  const at = focus.kind === 'filter' ? -1 : focus.index
  const last = count - 1
  const row = (index: number): BranchFocus => ({
    kind: 'row',
    index: Math.min(Math.max(index, 0), last),
  })

  switch (key) {
    case 'ArrowDown':
      return at >= last ? null : row(at + 1)
    case 'ArrowUp':
      // The transition the report is about. From the first row the answer is the field, not
      // "stay put" and not "wrap to the bottom".
      if (at < 0) return null
      return at === 0 ? FILTER : row(at - 1)
    case 'PageDown':
      return at >= last ? null : row(at + PAGE_ROWS)
    case 'PageUp':
      // A page jump from anywhere below the top lands *on the top row*, not in the field: a
      // jump that ends in a text box swallows the next thing typed. Only the row-0 case
      // continues into the field, so that holding Up and holding PageUp end in the same place.
      if (at < 0) return null
      return at === 0 ? FILTER : row(at - PAGE_ROWS)
    case 'Home':
      return at < 0 ? null : row(0)
    case 'End':
      return at < 0 ? null : row(last)
    default:
      return null
  }
}

/**
 * `focus` made safe against a list that changed underneath it.
 *
 * The classic bug this exists to prevent: the highlight points past the end and ⏎ checks out
 * `undefined`, or nothing, silently. It has two live routes here, and neither is exotic —
 * typing into the filter shortens the list under the highlight, and `cide://git-status` fires
 * whenever anything touches the refs, so a `git branch -d` typed into a terminal pane, a fetch
 * that prunes, or another window's delete all re-render this popup with fewer rows.
 *
 * Derived on every render rather than corrected in an effect: an effect would leave one frame
 * in which the popup drew a highlight on a row that is not there, and — worse — a keystroke
 * arriving in that frame would read the stale index.
 *
 * Landing on the *last* row rather than back in the field, because the user is walking the list
 * and yanking their place out into a text box mid-keypress is the more surprising of the two.
 */
export function clampFocus(focus: BranchFocus, count: number): BranchFocus {
  if (focus.kind === 'filter') return focus
  if (count === 0 || focus.index < 0) return FILTER
  return focus.index >= count ? { kind: 'row', index: count - 1 } : focus
}

/**
 * What ⏎ means right now.
 *
 * Three answers rather than a nullable branch name, because "there is nothing here" and "you
 * are already on that one" are different things to say and the second one has to be *said*: a
 * key that does nothing at all is how a user concludes the keyboard is not wired up, and the
 * branch you are standing on is the first row of the unfiltered list — the row Down lands on
 * first, every single time the popup opens.
 *
 * The filter case is the older behaviour, kept: ⏎ with the list narrowed to exactly one row
 * checks that row out. With more than one it does nothing rather than guessing, because a
 * checkout is not a gesture to resolve an ambiguity with — press Down and choose.
 */
export type EnterAction =
  | { kind: 'none' }
  | { kind: 'checkout'; name: string }
  | { kind: 'already'; name: string }

export function enterAction(rows: readonly BranchRef[], focus: BranchFocus): EnterAction {
  const entry = focus.kind === 'row' ? rows[focus.index] : rows.length === 1 ? rows[0] : undefined
  if (entry === undefined) return { kind: 'none' }
  if (entry.current) return { kind: 'already', name: entry.name }
  return { kind: 'checkout', name: entry.name }
}

/** The sentence for [`EnterAction`]`.already`. One place, so the popup and its check agree. */
export function alreadyOn(name: string): string {
  return `You are already on ${name}`
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
 * Which gesture the failure belongs to.
 *
 * Exactly one variant reads differently depending on the answer, and it is the one a user is
 * most likely to hit: `checkoutWouldOverwrite` is raised by a pull as well as by a checkout,
 * because both move the working tree onto another commit. Told to someone who pressed Ctrl+T
 * it used to read *"Switching to main would overwrite local changes"* — which names no switch
 * they asked for, and names the branch they are already standing on as the destination.
 *
 * A parameter rather than a second error variant in Rust: the refusal is genuinely the same
 * refusal, computed by the same `blockers()`, and splitting it in the crate would make
 * `cide-git` carry a fact about which button was pressed.
 */
export type GitOp = 'checkout' | 'pull'

/**
 * A `GitError` as a sentence, for everything the popup does not have a dedicated panel for.
 *
 * Written out per variant rather than falling back to `String(error)` because the wire form
 * is a tagged object: `String({kind: 'noUpstream', …})` is `[object Object]`, which is how a
 * control ends up appearing to do nothing at all. Anything genuinely unrecognised is still
 * shown — with its tag — rather than swallowed.
 */
export function explain(error: unknown, op: GitOp = 'checkout'): string {
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
      // Only files that genuinely differ between the two trees are in `paths` — a dirty file
      // the pull does not touch comes along untouched — so the sentence may not say "the tree
      // is dirty". It says what would be lost, which is the thing the user decides about.
      return op === 'pull'
        ? `Fast-forwarding ${name('branch')} would overwrite local changes${paths}`
        : `Switching to ${name('branch')} would overwrite local changes${paths}`
    }
    case 'branchExists':
      return `A branch named ${name('name')} already exists`
    case 'invalidBranchName':
      return `${name('name') === '' ? 'A branch name' : name('name')} is not a valid branch name — no spaces, no “..”, no trailing “.lock”`
    case 'branchNotMerged':
      // Not "its commits are on no other branch". Rust's test is `git branch -d`'s — is the
      // tip an ancestor of HEAD — so a branch already merged into `release` while you stand
      // on `main` lands here with nothing at stake. See `GitError::BranchNotMerged`.
      return `${name('name')} is not fully merged into the branch you are on. Anything only on it would be left unreachable.`
    case 'branchIsCurrent':
      return `${name('name')} is the branch you are on. Switch somewhere else first.`
    case 'noSuchBranch':
      return `${name('name')} is gone — someone deleted it since this list was drawn`
    case 'notFastForward':
      return `${name('branch')} is ${count('ahead')} ahead and ${count('behind')} behind its upstream. cide only fast-forwards; merge or rebase in a terminal.`
    case 'noUpstream':
      return `${name('branch')} has no upstream branch to pull from`
    case 'detachedHead':
      // Not "has no upstream", which is what this used to be folded into. A detached HEAD is
      // not a branch missing a setting; `git branch --set-upstream-to` cannot help, and the
      // sentence has to point at the fix that can.
      return `HEAD is detached at ${name('head')} — check out a branch before pulling`
    case 'noRemote':
      return `This repository has no remote named ${name('name')} — add one with \`git remote add\``
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

/**
 * Transport output as a single line.
 *
 * `git fetch` writes several — `From /srv/thing` then a line per ref — and the note bar is one
 * line high with no scroll. Joining with `·` rather than taking the first keeps the part that
 * says what moved, which is the last line, not the first.
 *
 * The cap is on characters and not on lines because a fetch of forty new branches is forty
 * lines, and a note that pushes the branch list off the popup is worse than a truncated one.
 */
function oneLine(text: string): string {
  const said = text
    .split(/[\r\n]+/)
    .map((part) => part.trim())
    .filter((part) => part !== '')
    .join(' · ')
  return said.length > 160 ? `${said.slice(0, 159)}…` : said
}

/**
 * A `FetchOutcome`, structurally.
 *
 * Declared rather than imported so this module keeps the property its foot-note states: no
 * value imports, and — because `verbatimModuleSyntax` is on — no import of the generated
 * types either, since `check-branches.mjs` compiles this one file with `types: []`. The check
 * pins the field names against `ui/src/ipc/generated.ts` instead, which is the same guarantee
 * arrived at from the other side.
 */
export interface FetchReport {
  remote: string
  advanced: number
  output: string
  branch: string
  oldOid: string
  newOid: string
  filesChanged: number
  insertions: number
  deletions: number
  commits: readonly { shortOid: string; summary: string; author: string }[]
  moreCommits: number
}

/** `1 commit` / `4 commits`, so every sentence here agrees on the wording. */
function commits(n: number): string {
  return `${n} ${n === 1 ? 'commit' : 'commits'}`
}

/** `1 file` / `12 files`. */
function files(n: number): string {
  return `${n} ${n === 1 ? 'file' : 'files'}`
}

/**
 * The diff totals as ` · 12 files +230 −41`, or `''` when there is nothing to total.
 *
 * A leading separator rather than a field the caller joins, so that a pull whose stats came
 * back empty produces no dangling `·`. `−` is U+2212, not a hyphen: this is a quantity, and
 * the same reasoning put `↑`/`↓` in `headLabel` rather than `^`/`v`.
 */
function shortstat(changed: number, insertions: number, deletions: number): string {
  if (changed === 0) return ''
  const plus = insertions > 0 ? ` +${insertions}` : ''
  const minus = deletions > 0 ? ` −${deletions}` : ''
  return ` · ${files(changed)}${plus}${minus}`
}

/**
 * What a fetch or a fast-forward pull has to say. Never empty: the user asked for network.
 *
 * Three answers, and they must be told apart at a glance, because they call for three
 * different next actions:
 *
 *   * **something came down** — `Fast-forwarded main 7 commits from origin · 12 files +230 −41`.
 *     The counts are the point: a person who has just pulled into a tree they are about to
 *     build wants to know whether to rebuild, and "ok" does not answer that.
 *   * **nothing came down** — `main is already up to date with origin`. The branch is named
 *     even though nothing moved, because with four repositories in a project that is the only
 *     thing saying which one just answered.
 *   * **it was refused** — not here at all. That is a `GitError` and `explain` is its sentence.
 *
 * A *fetch* names no branch (`branch` is empty; `cide_git::branch::fetch_with` fills in none of
 * the pull half) and keeps the older behaviour: git's own text, or "already up to date".
 *
 * `output` is trusted to be a *message*, which is why `cide_git::branch::fetch_with` no longer
 * forwards libgit2's sideband stream: that is a progress meter full of carriage returns, and
 * it used to arrive here and be printed verbatim as the one sentence a successful fetch got.
 * On the binary route it is also the *remote server's* text, so it stays inside `oneLine` —
 * shown, capped, and never parsed into anything.
 */
export function fetchNote(outcome: FetchReport): string {
  if (outcome.advanced > 0) {
    const moved = outcome.branch === '' ? '' : `${outcome.branch} `
    const stat = shortstat(outcome.filesChanged, outcome.insertions, outcome.deletions)
    return `Fast-forwarded ${moved}${commits(outcome.advanced)} from ${outcome.remote}${stat}`
  }
  if (outcome.branch !== '') {
    // A pull that took nothing. The transport may still have said something — other refs
    // moved — and that goes in the detail below rather than in the headline, where it would
    // displace the one fact the user asked about.
    return `${outcome.branch} is already up to date with ${outcome.remote}`
  }
  const said = oneLine(outcome.output)
  return said === '' ? `Already up to date with ${outcome.remote}` : said
}

/**
 * The body under `fetchNote` — what actually arrived, one commit per line.
 *
 * `''` when there is nothing to expand, which is what tells the surface not to draw the
 * disclosure at all.
 *
 * The list is already capped by Rust (`PULL_COMMIT_CAP`), so the overflow line reports a
 * number this side never had the rows for. That is deliberate: the alternative was putting
 * four hundred commits on the wire to drop three hundred and ninety here.
 *
 * When no commits came down the transport's own text takes the space instead — for a pull
 * that was already up to date, `Fetched 5 objects from origin` is the difference between
 * "nothing happened" and "your branch did not move but other refs did".
 */
export function fetchDetail(outcome: FetchReport): string {
  if (outcome.commits.length === 0) return oneLine(outcome.output)
  const lines = outcome.commits.map((c) => {
    const summary = c.summary === '' ? '(no summary)' : c.summary
    const author = c.author === '' ? '' : ` — ${c.author}`
    return `${c.shortOid}  ${summary}${author}`
  })
  if (outcome.moreCommits > 0) lines.push(`… and ${commits(outcome.moreCommits)} more`)
  return lines.join('\n')
}

/** One repository's answer, labelled. */
export interface RepoFetch {
  /** `RepoInfo.name`. Ignored when there is only one repository. */
  name: string
  outcome: FetchReport
}

/**
 * Several repositories' pulls as **one** notice.
 *
 * Aggregated rather than reported one toast per repository, for two reasons. One gesture
 * deserves one answer: Ctrl+T names no repository, so it acts on every one in the project
 * (the same argument `git.push` makes), and five toasts for one keystroke is noise. And the
 * failure mode of not aggregating is worse than noise — the notice surface collapses toasts
 * by identical text, so five submodules all answering *"Already up to date with origin"*
 * would have shown **one** toast and silently under-reported four repositories.
 *
 * With one repository this is exactly `fetchNote` / `fetchDetail`, so the common case reads
 * as though the multi-root machinery were not there.
 */
export function pullReport(results: readonly RepoFetch[]): { text: string; detail: string } {
  const first = results[0]
  if (results.length === 1 && first !== undefined) {
    return { text: fetchNote(first.outcome), detail: fetchDetail(first.outcome) }
  }
  if (first === undefined) return { text: '', detail: '' }

  const moved = results.filter((r) => r.outcome.advanced > 0)
  const total = (pick: (o: FetchReport) => number): number =>
    results.reduce((n, r) => n + pick(r.outcome), 0)

  // Every repository is listed, moved or not. A repository that was already up to date is an
  // answer to the question that was asked, and leaving it out would make the notice look like
  // the command had skipped it.
  const detail = results.map((r) => `${r.name}: ${fetchNote(r.outcome)}`).join('\n')

  if (moved.length === 0) {
    return { text: `All ${results.length} repositories are already up to date`, detail }
  }
  const stat = shortstat(
    total((o) => o.filesChanged),
    total((o) => o.insertions),
    total((o) => o.deletions),
  )
  const where = moved.length === results.length
    ? `all ${results.length} repositories`
    : `${moved.length} of ${results.length} repositories`
  return {
    text: `Fast-forwarded ${where} · ${commits(total((o) => o.advanced))}${stat}`,
    detail,
  }
}

/*
 * Runtime values only, no imports: `check-branches.mjs` compiles this file alone and loads the
 * emitted `.js` in node. Adding a value import — a store, an icon, `@/ipc/client` — breaks
 * that, and the check would start failing with a module-resolution error rather than telling
 * anyone why. The same note is at the foot of `sidebar/GitPanel/types.ts`.
 */
