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
 * and *delete on remote*. Their absence was a decision rather than a backlog, and the list is
 * kept current as items graduate off it:
 *
 * * **Merge into current** landed in M24 and is no longer on this list. The original reason
 *   for its absence — that it *ends in conflicts often enough that it is only usable with a
 *   conflict-resolution surface* — was answered by M20 (`panes/MergePane.tsx`, and the *Merge
 *   Conflicts* group in the commit panel); what was still missing, a chooser and a
 *   confirmation of its own, is now the row's "…" menu and the popup's `merge` panel, with
 *   `cide_git::merge` behind them. **Rebase current onto** is still absent for the remainder
 *   of that reason: it needs the same chooser-and-confirmation pair and it has not been
 *   designed. The *capability* is there — pull rebases — the gesture is not.
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
export type BranchAction = 'checkout' | 'newFrom' | 'merge' | 'rename' | 'delete'

/**
 * Which actions `entry` offers.
 *
 * Deriving the list rather than rendering five buttons and disabling three is the same rule
 * the rest of this app follows for menus: an item that is present and dead is a claim that
 * something is possible here. The exclusions are load-bearing:
 *
 * * the **current** branch cannot be checked out again (nothing would happen), merged into
 *   itself (nothing to take), or deleted (git refuses, and so does `cide_git::branch::delete`);
 * * a **remote-tracking** ref cannot be renamed or deleted, because it is a cache of the
 *   remote's names — the next fetch would put it straight back. *Checkout* on one is offered
 *   and means "create a local branch that tracks it", which is what git and IDEA both do.
 *   *Merge* on one is offered too and means the ref itself — `git merge origin/x` — which is
 *   how you take a fetched branch without creating a local copy of it first.
 * * **merge** also needs somewhere to merge *into*: a detached or unborn HEAD has no current
 *   branch, so the item vanishes rather than opening a panel whose only outcome is a refusal.
 *   `head` is required rather than optional so the compiler finds every call site that has to
 *   supply it. An operation in progress is deliberately *not* checked here — the popup's list
 *   can be minutes stale, and Rust's `OperationInProgress` sentence is the truthful answer,
 *   the same division of labour Pull uses.
 */
export function actionsFor(entry: BranchRef, head: BranchInfo | null): BranchAction[] {
  const actions: BranchAction[] = []
  if (!entry.current) actions.push('checkout')
  actions.push('newFrom')
  if (!entry.current && head !== null && !head.detached && !head.unborn) actions.push('merge')
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
 * `cide-git` carry a fact about which button was pressed. `merge` joined in M24 for the same
 * refusal again — a merge moves the working tree too, and `cide_git::merge` puts the *source*
 * ref in `branch`, so the sentence names the branch being merged rather than the one you are
 * standing on.
 */
export type GitOp = 'checkout' | 'pull' | 'merge'

/**
 * A list of paths in a sentence: `src/main.rs and src/lib.rs`, `a, b and 4 more`.
 *
 * Capped at three, because these go into a one-line notice and a conflict can name forty files.
 * The overflow says how many rather than truncating with an ellipsis: "and 37 more" is a fact
 * the user can act on — go and look in a terminal — and `a, b, c…` is not.
 */
function anded(items: readonly string[]): string {
  if (items.length === 0) return ''
  const shown = items.slice(0, 3)
  const rest = items.length - shown.length
  if (rest > 0) return `${shown.join(', ')} and ${rest} more`
  const last = shown[shown.length - 1] ?? ''
  return shown.length === 1 ? last : `${shown.slice(0, -1).join(', ')} and ${last}`
}

/**
 * A byte count, for the one refusal that has to quote a size.
 *
 * `number | bigint` in, because ts-rs maps Rust's `u64` to `bigint` while serde_json puts a
 * plain JSON number on the wire — so which of the two arrives here depends on whether anything
 * in the transport ever grows a reviver. Accepting both costs one line and removes a class of
 * "the limit is 0 bytes" that nobody would think to look for.
 */
function bytes(value: unknown): string {
  const n = typeof value === 'bigint' ? Number(value) : typeof value === 'number' ? value : 0
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/**
 * A `GitError` as a sentence, for everything the popup does not have a dedicated panel for.
 *
 * Written out per variant rather than falling back to `String(error)` because the wire form
 * is a tagged object: `String({kind: 'noUpstream', …})` is `[object Object]`, which is how a
 * control ends up appearing to do nothing at all. Anything genuinely unrecognised is still
 * shown — with its tag — rather than swallowed.
 *
 * # The register, which is a rule rather than a style
 *
 * Every arm here says three things in this order: **what happened**, **why**, and **what to do
 * instead**. The third is the one that is easy to drop and the one users need — cide performs a
 * deliberately small subset of git, so a good half of these refusals end in "do it in a
 * terminal", and a refusal that does not say that reads as a bug in cide rather than as a
 * boundary it chose. `notHead` is the clearest case: *amend is not implemented for older
 * commits* is a defect report; *amending an older commit is an interactive rebase, which cide
 * does not do* is a design, and it names the command that does.
 *
 * The arms below `notARepository` are the commit actions (M19). They are grouped by the gesture
 * they belong to rather than sorted, because that is how they are read when one of them fires.
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
  /** A path list out of the detail, already filtered to strings. */
  const paths = (key: string): string[] => {
    const value = field(parsed, key)
    return Array.isArray(value) ? value.filter((p): p is string => typeof p === 'string') : []
  }
  /** `Reverting` / `Cherry-picking`, from a `ReplayOp` in the detail. */
  const replaying = (): string => (name('op') === 'revert' ? 'Reverting' : 'Cherry-picking')

  switch (parsed.kind) {
    case 'checkoutWouldOverwrite': {
      const refusal = refusalOf(error)
      const paths = refusal === null ? '' : `: ${refusal.paths.join(', ')}`
      // Only files that genuinely differ between the two trees are in `paths` — a dirty file
      // the pull does not touch comes along untouched — so the sentence may not say "the tree
      // is dirty". It says what would be lost, which is the thing the user decides about.
      if (op === 'merge') {
        return `Merging ${name('branch')} would overwrite local changes${paths}`
      }
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
      // Reworded in M20. *"cide only fast-forwards; merge or rebase in a terminal"* stopped
      // being true — this now fires only when the user has chosen **Fast-forward only**, so it
      // has to name the setting that caused it. Sending somebody to a terminal for something
      // the app does on the next line is the worst kind of stale refusal.
      return `${name('branch')} is ${count('ahead')} ahead and ${count('behind')} behind its upstream, and your pull preference is fast-forward only. Change it in Settings › Git, or merge or rebase from the pull dialog.`
    case 'pullNeedsStrategy':
      // Reachable only as a fallback: `pullStrategyModel.divergenceOf` catches this variant and
      // opens the dialog instead. It still needs a sentence, because a *second* one raised while
      // answering the first is deliberately not turned into another dialog — see `dispatch.ts`.
      return `${name('branch')} is ${count('ahead')} ahead and ${count('behind')} behind its upstream. Pull again to choose merge or rebase.`
    case 'rebaseWouldDropMerges':
      return `Rebasing ${name('branch')} would drop merge commits, discarding the conflict resolutions recorded in them. Merge instead, or rebase in a terminal with \`--rebase-merges\`.`
    case 'rebaseTooLong':
      return `${name('branch')} has ${count('ahead')} commits to replay, more than the ${count('limit')} cide rebases in one go. Merge instead, or rebase in a terminal.`
    case 'unrelatedHistories':
      return `${name('branch')} and ${name('upstream')} share no common commit, so there is nothing to merge onto. Check you are on the branch you meant to be.`
    case 'notConflicted':
      return `${name('path')} is not conflicted — another window may have resolved it already.`
    case 'stagesGone':
      return `${name('path')} cannot be un-resolved: resolving a path is what removes the conflicting versions, and git keeps no copy.`
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

    // --- amend ------------------------------------------------------------------------------
    case 'notHead':
      // Names the commit that *is* the tip, because the usual cause is that the log page went
      // stale — another window committed, or a `git commit` was typed into a terminal pane —
      // and "a1b2c3d is not the last commit" without saying which one is leaves the user
      // staring at a row that looks like the top of the list.
      return `${name('oid')} is not the last commit — ${name('head')} is. Amending it means rewriting history, which cide does not do — use \`git rebase -i\` in a terminal.`

    // --- revert and cherry-pick ---------------------------------------------------------------
    case 'replayWouldConflict': {
      const where = paths('paths')
      const list = where.length === 0 ? '' : ` in ${anded(where)}`
      // Names the files, because the next decision — is this worth doing by hand — is entirely
      // about which files they are. And it says *nothing was changed*: `cide_git::replay` does
      // the merge in memory and refuses before writing, so the working tree is untouched and a
      // user who has met git's half-applied cherry-pick will otherwise go looking for one.
      return `${replaying()} ${name('oid')} would conflict${list}. Nothing was changed.`
    }
    case 'mergeNeedsMainline':
      // The picker (`chrome/logActions.ts::mainlineChoices`) is what the log actually shows for
      // this one. This sentence is the fallback for every other caller, and for the case where
      // the parents did not survive the trip.
      return `${name('oid')} is a merge, so it has more than one “before”. Reverting it means choosing which parent to keep — git calls that the mainline, and it will not guess.`
    case 'notAMerge':
      return `${name('oid')} is not a merge, so there is no mainline to choose. Ask again without one.`
    case 'emptyReplay':
      // "Would change nothing" and not "failed": this is the honest description of reverting a
      // commit whose changes are already undone, and it is a normal thing to try.
      return `${replaying()} ${name('oid')} would change nothing — the tree already matches. Nothing was committed.`

    // --- tags ---------------------------------------------------------------------------------
    case 'tagExists':
      // Names where the existing tag points, because that is what the user needs to decide
      // whether moving it is safe. The force path is `chrome/logActions.ts::forceTagConfirm`.
      return `A tag named ${name('name')} already exists and points at ${name('oid')}. Move it, or pick another name.`
    case 'invalidTagName':
      // Same shape as `invalidBranchName` above, with the extra characters git rejects in a
      // refname spelled out — a tag called `v1.2^` fails for a reason nothing on screen says.
      return `${name('name') === '' ? 'A tag name' : name('name')} is not a valid tag name — no spaces, no “..”, no trailing “.lock”, and none of \` ~ ^ : ? * [ \``

    // --- resolving what the user pointed at -----------------------------------------------------
    case 'noSuchCommit':
      return `There is no commit ${name('rev')} in this repository — it may have been rewritten or garbage-collected since this page was drawn.`
    case 'noSuchRevision':
      return `Nothing in this repository is named ${name('rev')} — the branch, tag or commit it pointed at is gone.`
    case 'badRevspec':
      // git's own parser message is carried through: it is specific ("unknown revision or path
      // not in the working tree") in a way nothing written here could be, and the user typed
      // the spec so they can act on it.
      return `${name('spec')} is not something git can resolve: ${name('detail')}`
    case 'notACommit':
      // `kind` is git's object kind — `tree`, `blob`, `tag`. Naming it is the difference between
      // "that did not work" and "you pointed at a file".
      return `${name('spec')} resolves to a ${name('kind')}, not a commit. Only a commit can be shown here.`
    case 'ambiguousRev':
      return `${name('spec')} matches more than one object in this repository — type a few more characters of the oid.`

    // --- file history and blame ------------------------------------------------------------------
    case 'notTracked':
      return `${name('path')} is not tracked by git, so it has no history yet — commit it first.`
    case 'fileTooLarge':
      // Both numbers. "Too large" without a limit is a refusal the user cannot plan around, and
      // the limit is a constant in `cide-git` that nothing else on screen states.
      return `${name('path')} is ${bytes(field(parsed, 'bytes'))}, past the ${bytes(field(parsed, 'limit'))} cide will read into a diff — open it in a terminal.`

    // --- paging ------------------------------------------------------------------------------------
    case 'staleLogCursor':
      // Carries no detail on the wire, and needs none: the whole content of this error is "the
      // history moved under the page you were reading". The action is a refresh, and saying so
      // is the difference between an error and an instruction.
      return 'The log moved while that page was loading. Refresh it and try again.'

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
  /** What the pull actually did. Absent for a plain fetch and for a pull that took nothing. */
  strategy?: 'fastForward' | 'merge' | 'rebase' | undefined
  /** How many of your own commits a rebase replayed. */
  rewritten: number
  /** Local commits a rebase dropped because replaying them changed nothing. */
  skipped: number
  /** Paths left conflicted. Non-empty is a success with work attached, not a failure. */
  conflicts: readonly string[]
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
  /*
   * Conflicts first, because they are the only outcome with work attached.
   *
   * A merge that stopped still *took* every one of its commits, so the counts below are all
   * true — and leading with them would bury the one fact the user has to act on under a
   * sentence that reads like success. This is the arm that says what to do next.
   */
  if (outcome.conflicts.length > 0) {
    const verb = outcome.strategy === 'rebase' ? 'Rebasing' : 'Merging'
    const n = outcome.conflicts.length
    return `${verb} ${outcome.branch} — ${n} file${n === 1 ? '' : 's'} to resolve: ${anded(outcome.conflicts)}`
  }
  if (outcome.advanced > 0) {
    const moved = outcome.branch === '' ? '' : `${outcome.branch} `
    const stat = shortstat(outcome.filesChanged, outcome.insertions, outcome.deletions)
    /*
     * The verb is the strategy's, not "fast-forwarded" for all three.
     *
     * This function predates merge and rebase and said *"Fast-forwarded main 1 commit from
     * origin"* whatever had happened — which is not a wording quibble: a user who chose *Rebase*
     * in the dialog and was then told their branch had been fast-forwarded has been told their
     * answer was ignored, and the history they are about to push is not the shape the sentence
     * describes.
     */
    if (outcome.strategy === 'merge') {
      return `Merged ${commits(outcome.advanced)} from ${outcome.remote} into ${outcome.branch}${stat}`
    }
    if (outcome.strategy === 'rebase') {
      // Both numbers, because a rebase does two things and one of them is to *your* commits.
      // The dropped ones are named too — `git rebase` drops them silently, and a user whose
      // three commits became two goes looking for the lost one.
      const mine =
        outcome.rewritten === 0 ? '' : `, replaying ${commits(outcome.rewritten)} of yours`
      const dropped =
        outcome.skipped === 0 ? '' : ` (${commits(outcome.skipped)} already upstream, dropped)`
      return `Rebased ${outcome.branch} onto ${commits(outcome.advanced)} from ${outcome.remote}${mine}${dropped}${stat}`
    }
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
  // "Updated", not "Fast-forwarded": with several repositories the strategies can differ — one
  // fast-forwards while another merges — so the aggregate uses the word that is true of all of
  // them, and the per-repository detail below says which was which.
  return {
    text: `Updated ${where} · ${commits(total((o) => o.advanced))}${stat}`,
    detail,
  }
}

// --- merge into current --------------------------------------------------------------------

/**
 * A `MergeOutcome`, structurally. Declared rather than imported for `FetchReport`'s reason:
 * the check pins these field names against `ui/src/ipc/generated.ts` instead.
 */
export interface MergeReport {
  source: string
  branch: string
  fastForward: boolean
  oldOid: string
  newOid: string
  advanced: number
  filesChanged: number
  insertions: number
  deletions: number
  commits: readonly { shortOid: string; summary: string; author: string }[]
  moreCommits: number
  /** Paths left conflicted. Non-empty is a success with work attached, not a failure. */
  conflicts: readonly string[]
}

/**
 * The merge panel's question and its one-sentence answer to "what will this do".
 *
 * The body states the three outcomes because a merge is the one branch action whose result is
 * not a rename of something on screen: it can move the tree without a commit, write a commit,
 * or stop half-way — and a person deciding whether to press the button is deciding about all
 * three. It also says what the merge does *not* do (push), because "merge" in a popup that
 * also offers Push is one click away from meaning more than it does.
 */
export function mergeConfirm(source: string, into: string): { title: string; body: string } {
  return {
    title: `Merge ${source} into ${into}?`,
    body:
      `Fast-forwards ${into} when it can, otherwise writes a merge commit. ` +
      'A conflict opens the resolver with real git state — abortable at any point. ' +
      'Nothing is pushed.',
  }
}

/**
 * What a completed merge has to say. Never empty: the user confirmed a panel to get here.
 *
 * The same contract as `fetchNote`, with the source branch where the remote was: conflicts
 * lead because they are the only outcome with work attached, the counts answer "what did I
 * just take", and *already contains* names both branches because the popup may be acting on a
 * different repository from the one on screen.
 */
export function mergeNote(outcome: MergeReport): string {
  if (outcome.conflicts.length > 0) {
    const n = outcome.conflicts.length
    return `Merging ${outcome.source} into ${outcome.branch} — ${n} file${n === 1 ? '' : 's'} to resolve: ${anded(outcome.conflicts)}`
  }
  const stat = shortstat(outcome.filesChanged, outcome.insertions, outcome.deletions)
  if (outcome.advanced === 0) {
    return `${outcome.branch} already contains ${outcome.source}`
  }
  if (outcome.fastForward) {
    // Named as what it was: a fast-forward made no commit, and a person about to push wants
    // to know their history is a straight line — `fetchNote`'s argument for verb honesty.
    return `Fast-forwarded ${outcome.branch} ${commits(outcome.advanced)} to ${outcome.source}${stat}`
  }
  return `Merged ${commits(outcome.advanced)} from ${outcome.source} into ${outcome.branch}${stat}`
}

/**
 * A `PushOutcome`, structurally.
 *
 * Declared rather than imported for [`FetchReport`]'s reason, and named `PushInfo` rather than
 * `PushReport` for one more: `PushReport` and [`pushReport`] would differ only in case, which
 * is the near-collision `ui/scripts/check-casing.mjs` exists to catch on a case-insensitive
 * filesystem. The field names are pinned against `ui/src/ipc/generated.ts` by
 * `check-branches.mjs`, which is the same guarantee arrived at from the other side.
 */
export interface PushInfo {
  remote: string
  refspec: string
  shelledOut: boolean
  output: string
  /** The destination branch's short name. Empty when the refspec named something else. */
  branch: string
  /** How many commits the remote gained. Zero when it was already up to date. */
  pushed: number
  /** Where the remote ref was before. **Empty when the remote had no such ref.** */
  oldOid: string
  newOid: string
}

/**
 * What a push has to say. Never empty: the user asked for network.
 *
 * Three answers, told apart at a glance, the same rule `fetchNote` follows and for the same
 * reason — they call for different next actions:
 *
 *   * **a branch that did not exist on the remote** — `Published feature to origin · 3 commits`.
 *     `oldOid` is empty only for a `--set-upstream` publish, and *"Pushed 3 commits"* is true
 *     but misses the thing the user actually just did: the branch is now visible to everybody
 *     else, which is the fact they will act on next.
 *   * **nothing went up** — `main is already up to date on origin`. The branch is named for
 *     `pullReport`'s reason: with four repositories it is the only thing that says which one
 *     answered.
 *   * **commits went up** — `Pushed 3 commits to origin/main`.
 *
 * A refusal is not here at all. That is a `GitError` and `explain` is its sentence.
 */
export function pushNote(outcome: PushInfo): string {
  const where = outcome.branch === '' ? outcome.remote : `${outcome.remote}/${outcome.branch}`
  if (outcome.oldOid === '' && outcome.pushed > 0) {
    return `Published ${outcome.branch} to ${outcome.remote} · ${commits(outcome.pushed)}`
  }
  if (outcome.pushed === 0) {
    const said = oneLine(outcome.output)
    if (outcome.branch !== '') return `${outcome.branch} is already up to date on ${outcome.remote}`
    return said === '' ? `Already up to date on ${outcome.remote}` : said
  }
  return `Pushed ${commits(outcome.pushed)} to ${where}`
}

/**
 * The body under `pushNote`: git's own text, multi-line.
 *
 * **Not through `oneLine`**, and that is the one place this differs from `fetchDetail`.
 * `Notice.detail` is documented as multi-line and rendered pre-wrapped, and `oneLine`'s
 * 160-character cap would truncate a GitHub push's
 * `remote: Create a pull request for 'x' on GitHub: https://…` mid-URL — which is the single
 * most useful thing a push ever prints. Collapsed, trimmed and capped instead, so a chatty
 * server cannot turn one toast into a page.
 *
 * Untrusted and never parsed, exactly as `fetchDetail`'s is: on the binary route this is the
 * remote server's own text, written by whoever runs that server.
 */
export function pushDetail(outcome: PushInfo): string {
  const lines = outcome.output
    .split(/[\r\n]+/)
    .map((line) => line.trim())
    .filter((line) => line !== '')
  const said = lines.join('\n')
  return said.length > 600 ? `${said.slice(0, 599)}…` : said
}

/** One repository's push, labelled. `RepoFetch`'s shape, for `RepoFetch`'s reason. */
export interface RepoPush {
  /** `RepoInfo.name`. Ignored when there is only one repository. */
  name: string
  outcome: PushInfo
}

/**
 * Several repositories' pushes as **one** notice.
 *
 * Aggregated for exactly `pullReport`'s two reasons, restated because they are what make this
 * function necessary rather than decorative. One gesture deserves one answer: `git.push` names
 * no repository, so it acts on every one in the project. And the failure mode of not
 * aggregating is worse than noise — `notices.admit` collapses toasts by identical text, so five
 * submodules all answering *"already up to date on origin"* would show **one** toast and
 * silently speak for five.
 *
 * With one repository this is exactly `pushNote` / `pushDetail`, so the common case reads as
 * though the multi-root machinery were not there.
 */
export function pushReport(results: readonly RepoPush[]): { text: string; detail: string } {
  const first = results[0]
  if (results.length === 1 && first !== undefined) {
    return { text: pushNote(first.outcome), detail: pushDetail(first.outcome) }
  }
  if (first === undefined) return { text: '', detail: '' }

  const moved = results.filter((r) => r.outcome.pushed > 0)
  // Every repository is listed, moved or not. One that was already up to date is an answer to
  // the question that was asked, and leaving it out would make the notice look like the command
  // had skipped it.
  const detail = results.map((r) => `${r.name}: ${pushNote(r.outcome)}`).join('\n')

  if (moved.length === 0) {
    return { text: `All ${results.length} repositories are already up to date`, detail }
  }
  const total = moved.reduce((n, r) => n + r.outcome.pushed, 0)
  const where =
    moved.length === results.length
      ? `all ${results.length} repositories`
      : `${moved.length} of ${results.length} repositories`
  return { text: `Pushed ${commits(total)} from ${where}`, detail }
}

/*
 * Runtime values only, no imports: `check-branches.mjs` compiles this file alone and loads the
 * emitted `.js` in node. Adding a value import — a store, an icon, `@/ipc/client` — breaks
 * that, and the check would start failing with a module-resolution error rather than telling
 * anyone why. The same note is at the foot of `sidebar/GitPanel/types.ts`.
 */
