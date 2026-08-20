/**
 * Every decision the commit log makes about what a row says, with none of the React it says it
 * in. (M19)
 *
 * Import-free but for types, exactly as `chrome/branchModel.ts` is, so
 * `ui/scripts/check-log.mjs` can compile it standalone and execute it. What belongs here is
 * anything that can be wrong on its own: the order of the ref chips, where a long list of them
 * is cut, how a timestamp is rendered, and — most of all — **which sentence each empty state
 * shows**, because two states that say the same thing are indistinguishable to the user and
 * neither of them is checkable from a screenshot.
 *
 * # Dates are absolute, and that is a house rule rather than a preference
 *
 * The mock this feature was drawn from showed "2h ago". `sidebar/GitPanel/ShelfList.tsx` settled
 * the question the other way and gave the reason: *"A relative string has to be recomputed to
 * stay true, and a list that silently goes stale is worse than a date that is always right."* A
 * log is refreshed on demand and can sit on screen for an hour, so it is the surface that
 * argument bites hardest on. [`when`] therefore renders the platform's own short form, and the
 * full timestamp goes in the row's `title` where it can be read exactly.
 *
 * # Both halves of the filter live here, and that is the point of putting them here
 *
 * A filter is asked over the wire, because a hundred-thousand-commit history cannot be narrowed
 * inside the one page that happens to be loaded — that is what `LogQuery::text` and
 * `LogQuery::author` are for, and `cide_git::log` applies them inside the same scan budget the
 * walk is bounded by. But a wire round trip is debounced by [`FILTER_DEBOUNCE_MS`], and a filter
 * box that does nothing for a fifth of a second per keystroke feels broken. So the text box
 * *also* narrows the rows already on screen, immediately, and the wire answer replaces that a
 * moment later.
 *
 * Two passes over one question is exactly the arrangement that drifts. Keeping
 * [`matchesText`] — the local rule — in the same file as [`filterOverrides`] — what the wire is
 * asked — is what makes the divergence between them a thing a reader can see and
 * `check-log.mjs` can pin, rather than a thing discovered when a row flickers in and out.
 */
import type {
  CommitPage,
  CommitRow,
  FsChange,
  GraphOff,
  LogQuery,
  LogRefs,
  LogStop,
  RefChip,
  RepoId,
  RepoInfo,
} from '@/ipc/generated'

/** The most ref chips a row draws before the rest become a `+n`. */
export const MAX_CHIPS = 3

/**
 * Chips in drawing order, capped.
 *
 * HEAD first, then local branches, then remote-tracking, then tags. The current branch is the row
 * every other one is relative to, which is the same ordering — and the same reason —
 * `branchModel::visibleBranches` puts it first in the popup.
 *
 * The cap is not cosmetic: a release commit can carry forty tags, and forty chips push the
 * subject — the only column anyone scans — off the row entirely. The overflow chip keeps the
 * count honest rather than silently dropping them.
 */
export function orderChips(refs: readonly RefChip[]): { shown: RefChip[]; more: number } {
  const rank: Record<RefChip['kind'], number> = {
    head: 0,
    localBranch: 1,
    remoteBranch: 2,
    tag: 3,
  }
  const sorted = [...refs].sort((a, b) => rank[a.kind] - rank[b.kind] || a.name.localeCompare(b.name))
  return { shown: sorted.slice(0, MAX_CHIPS), more: Math.max(0, sorted.length - MAX_CHIPS) }
}

/**
 * The three fields a chip's decision is allowed to see. (M21)
 *
 * Narrower than [`RefChip`] deliberately, and **the field that is missing is the whole point:
 * `name` is not here.** A chip carries both spellings — `name` is the shorthand the row draws
 * (`main`) and `full` is the refname (`refs/heads/main`) — and the shorthand is genuinely
 * ambiguous: `main` is either `refs/heads/main` or `refs/remotes/origin/main`, and
 * `cide_git::log::find_branch_tip` resolves the ambiguity by trying local first. A chip that
 * re-rooted the walk on the wrong one of those is a bug the user cannot see, because the answer
 * is not an error: it is a plausible list of real commits off a different branch. Leaving `name`
 * out of the type makes sending it *impossible* rather than merely discouraged — [`chipTarget`]
 * cannot reach for the wrong field, because it was never handed it.
 *
 * A `RefChip` is structurally assignable to this, so the view passes the wire value straight in
 * and nothing has to be mapped.
 */
export interface RefChipLike {
  readonly kind: RefChip['kind']
  /**
   * The full refname — `refs/heads/main`, `refs/remotes/origin/main`, `refs/tags/v1.2.0`.
   *
   * Or the literal string `HEAD`, which is what `cide_git::log::chips` puts on the chip it emits
   * for a **detached** HEAD. That chip exists only in the detached case: an attached HEAD is
   * already named by the branch chip beside it, which carries `current: true` instead.
   */
  readonly full: string
  /**
   * Whether HEAD resolves through this ref — `cide_git::log::chips` compares the refname against
   * `repo.head()`. Read by [`chipTarget`] and by nothing else here; see its `head` arm.
   */
  readonly current: boolean
}

/**
 * Whether the walk already starts at the ref this chip names.
 *
 * Deliberately about **what was asked for**, not about which commit the answer happens to land
 * on. Two chips on one row — `main` and `origin/main` at the same oid, which is the state a
 * repository is in for the minute after a push — name two different questions that agree today
 * and will disagree after the next fetch. This module holds chips, not oids, and a rule that
 * guessed "same commit, therefore same request" would be wrong the moment they diverge and would
 * be wrong silently.
 *
 * The `head` arm is the one place `current` earns its passage on the wire. `LogRefs::Head` is
 * *defined* as the ref HEAD points at — `cide_git::log::seed_from_refs` seeds it with
 * `repo.head().peel_to_commit()` — so the chip flagged `current` is not "a chip at the same
 * commit", it is literally the same request spelled a second way. That is a fact and not an
 * inference, which is why it is safe here where an oid comparison would not be.
 *
 * The `branch` arm mirrors `find_branch_tip`'s local-then-remote lookup, because that is what
 * `LogRefs::Branch { name }` actually resolves through, and it is how a `main` picked out of the
 * filter bar's `<select>` and a `main` chip on the row end up meaning one thing. It inherits that
 * function's one ambiguity: with both a local `foo` and a remote `refs/remotes/foo` present, the
 * remote chip is treated as already-rooted while the walk is really on the local one. The
 * alternative — calling it live — is a chip that silently re-roots onto a *different* ref while
 * the bar keeps saying `foo`, and of the two lies a spurious "nothing to do here" is the one the
 * user can recover from by using the `<select>`.
 *
 * `all` is false for every chip: `LogRefs::All` seeds every tip in the repository, so narrowing
 * to one of them is a real change and no chip is "where the walk starts".
 */
function alreadyRooted(chip: RefChipLike, current: LogRefs): boolean {
  switch (current.kind) {
    case 'head':
      return chip.current
    case 'rev':
      return current.spec === chip.full
    case 'branch':
      return (
        chip.full === `refs/heads/${current.name}`
        || chip.full === `refs/remotes/${current.name}`
      )
    case 'all':
      return false
  }
}

/**
 * What clicking a ref chip asks for, or `null` when the chip is not a control at all. (M21)
 *
 * `null` is *one* decision expressed once, and that is why this returns it rather than the view
 * asking a second question: `LogView` renders a `<span>` when this is `null` and a `<button>`
 * when it is not. A dead `<button>` — listed, hovering like a control, doing nothing when pressed
 * — is the exact state `cide-core::commands` refuses to represent for menu items, for the same
 * reason: nothing on screen distinguishes it from a working one until it is clicked.
 *
 * # Why every named ref goes out as `LogRefs::Rev { spec: chip.full }`
 *
 * `Rev` and not `Branch { name }`, for all three of local, remote-tracking and tag:
 *
 * * **A tag has no `Branch` arm at all.** `find_branch_tip` looks in `refs/heads` and
 *   `refs/remotes`, so `Branch { name: 'v1.2.0' }` answers `LogStop::NoSuchRef` and the panel
 *   shows an empty list with a footnote. `revparse` peels an annotated tag to its commit, which
 *   is what the chip meant.
 * * **`Branch` re-introduces exactly the ambiguity `full` exists to settle.** git permits a local
 *   branch literally named `origin/main` (`refs/heads/origin/main`), and `find_branch_tip` tries
 *   local first — so the *remote* chip's name would resolve to the *local* branch. `revparse`
 *   given a full refname cannot pick the wrong ref.
 * * One rule instead of three is also what keeps this checkable: `check:log` asserts the spec is
 *   `chip.full` for every kind, and there is no per-kind arm for a future edit to get wrong.
 *
 * A `Rev` walk keeps its graph — `cide_git::log::graph_off` returns `Rerooted` for a resumed
 * *cursor*, not for a re-rooted ref — so this costs the lanes nothing.
 *
 * # The `head` chip re-roots on `LogRefs::Head`, and does not go out as a revspec
 *
 * `Rev { spec: 'HEAD' }` would resolve to the same commit and would still be the wrong value.
 * `isFiltered` counts any non-`head` branch as a filter, so it would light up *Clear filters* and
 * push the `<select>` onto its synthetic "Revision HEAD" option — a visibly filtered bar in front
 * of a walk that is byte-for-byte the default one. And the two disagree in the one place it
 * matters: `seed_from_refs`'s `Head` arm treats an unborn HEAD as an empty page on purpose ("a
 * new project must be able to open its log without a dialog"), while `revparse("HEAD")` on an
 * unborn repository is a `BadRevspec` the tab would render as a failure.
 *
 * So the head chip is the way *back*: it is the only chip that can return the walk to the
 * default, which is the same thing the `<select>`'s first option ("Current branch") does. The
 * option that lost was making it permanently inert on the grounds that "HEAD is where the walk
 * usually already is" — true, and already handled, because in that state [`alreadyRooted`] makes
 * it inert anyway. Making it inert *unconditionally* would have thrown away the one case where it
 * has something to say: a user who re-rooted onto a tag three clicks ago and wants their own
 * checkout back.
 *
 * # The `+n` overflow chip is not passed here at all
 *
 * It names a count, not a ref, so there is nothing for it to re-root on; `LogView` draws it as a
 * `<span>` with its own title and never calls this. The empty-`full` guard below is not for that
 * chip — it is for the next person who wires one up without reading this: `Rev { spec: '' }` is a
 * `revparse("")`, which comes back as `BadRevspec` and replaces the list with a failure sentence
 * in answer to a click that should have done nothing. (Making the `+n` chip *expand* the row to
 * its full list of refs is a reasonable feature and a different one; it is not this.)
 */
export function chipTarget(chip: RefChipLike, current: LogRefs): LogRefs | null {
  if (chip.full === '') return null
  if (alreadyRooted(chip, current)) return null
  return chip.kind === 'head' ? { kind: 'head' } : { kind: 'rev', spec: chip.full }
}

/**
 * A chip's tooltip: the refname it stands for, and what pressing it does.
 *
 * The refname stays the first line, unchanged, because that is the disambiguation the chip was
 * carrying `full` for in the first place — a row showing `main` and `main` is two chips whose
 * only distinguishing mark is this string.
 *
 * The second line exists because the affordance is otherwise invisible. An inert chip is a
 * `<span>` and a live one is a `<button>`, which differ by a cursor and a hover tint and by
 * nothing at all in a screenshot; "The log already starts here." is the sentence that stops the
 * inert case reading as a control that is broken. It is derived from [`chipTarget`] rather than
 * from a second predicate, so the tooltip cannot claim one thing while the click does another.
 */
export function chipTitle(chip: RefChipLike, current: LogRefs): string {
  const target = chipTarget(chip, current)
  if (target === null) return `${chip.full}\nThe log already starts here.`
  if (target.kind === 'head') return `${chip.full}\nStart the log at the current checkout.`
  return `${chip.full}\nStart the log here.`
}

/**
 * The date a row shows: the platform's short form, in the viewer's locale.
 *
 * `seconds` is unix seconds, which is how every timestamp on this wire is spelled (`BranchRef`
 * and `ShelfEntry` both) — but ts-rs renders an `i64` as a **`bigint`**, so it arrives as one and
 * has to be narrowed before `Date` will take it. Narrowed here, once, rather than at each call
 * site: `Number(bigint)` is lossless for every second this side of the year 285616, and doing it
 * per caller is how one of them forgets and gets `NaN` in a date field that then renders
 * "Invalid Date" with no error anywhere.
 */
export function when(seconds: bigint | number, now: number): string {
  const date = new Date(Number(seconds) * 1000)
  const sameYear = date.getFullYear() === new Date(now).getFullYear()
  return date.toLocaleDateString(undefined, {
    day: '2-digit',
    month: 'short',
    ...(sameYear ? {} : { year: 'numeric' }),
  })
}

/** The exact timestamp, for a row's tooltip. Never truncated and never relative. */
export function exactly(seconds: bigint | number): string {
  return new Date(Number(seconds) * 1000).toLocaleString()
}

/** A row's tooltip: who, when, and the oid, so the short hash on screen can be resolved. */
export function rowTitle(row: CommitRow): string {
  return `${row.oid}\n${row.author} <${row.authorEmail}>\n${exactly(row.authored)}`
}

/**
 * Merge a page onto what is already loaded, dropping anything already there.
 *
 * De-duplicated by oid because the walk can legitimately repeat a row across a page boundary
 * under real clock skew — a descendant committed with an earlier timestamp than its ancestor.
 * `cide_git::log` bounds that with a recent-oid ring and cannot eliminate it without carrying
 * every emitted oid; this is the other half of that trade, and it is what stops a repeat becoming
 * a duplicate React key.
 */
export function mergePage(loaded: readonly CommitRow[], page: CommitPage): CommitRow[] {
  const seen = new Set(loaded.map((r) => r.oid))
  const out = [...loaded]
  for (const row of page.commits) {
    if (seen.has(row.oid)) continue
    seen.add(row.oid)
    out.push(row)
  }
  return out
}

/** Whether there is more to fetch. `resume` is the authority, never `stop`. */
export function hasMore(page: CommitPage | null): boolean {
  return page !== null && page.resume !== null
}

/**
 * What the button at the foot of the list says.
 *
 * **`budget` is not the end of history**, and this is the one place that distinction becomes
 * visible. The walk inspected its whole budget without filling the page — which on a filtered
 * search over a deep repository is the *expected* outcome — so a list that drew that as "no more
 * commits" would silently lose the answer the user was looking for.
 */
export function moreLabel(page: CommitPage): string {
  if (page.stop === 'budget') {
    return `Searched ${page.scanned.toLocaleString()} commits — keep looking?`
  }
  if (page.stop === 'shallow') return 'This clone is shallow — the history stops here'
  return 'Load more'
}

/** Why a list is empty, or `null` when it is not. Every branch is a different sentence. */
export interface LogStatusInput {
  readonly loading: boolean
  readonly failed: string | null
  /**
   * The project has no git repository under any of its roots.
   *
   * A state of its own and not a failure, because the *backend* answer to it is a failure —
   * `git_log` with an empty scope rejects with `GitError::NoSuchProject`, whose `detail` is an
   * object, and a panel that printed that would show `[object Object]`. `LogTab` therefore does
   * not ask the question at all, which leaves the sentence to be produced here, where every
   * other sentence lives.
   */
  readonly noRepo: boolean
  /** How many rows are **on screen** — after the local narrow, not before it. */
  readonly rows: number
  readonly filtered: boolean
  readonly path: string | null
  readonly stop: LogStop | null
}

/**
 * The one sentence an empty list shows, or `null` when there is a list to show instead.
 *
 * # Why every branch has to say something different
 *
 * Six of these describe a page with no rows in it, and to the user they are six different
 * situations with six different next moves: wait, clear the filter, keep looking, deepen the
 * clone, pick another branch, or make the first commit. A view that collapsed any two of them
 * would leave a user doing the wrong thing with no way to find out — and it would be invisible
 * in a screenshot, which is why the distinctness is asserted in `check-log.mjs` rather than
 * trusted.
 *
 * # The order is the order of certainty, and `budget` outranking the filters is the load-bearing
 * part
 *
 * A walk that stopped on [`LogStop::Budget`] has **not** looked at the whole history. Saying "no
 * commit matches these filters" there is a false statement about the repository — it is
 * precisely the silent loss the scan budget's whole design exists to make visible, and the
 * *expected* outcome of a narrow filter over a deep repository. Same for a path: "nothing in
 * this history touches src/main.rs" is a claim about the whole history and only `exhausted`
 * supports it. So the stop-driven branches come first, and the two "we looked and found
 * nothing" sentences are only reachable once the walk actually finished looking.
 */
export function logStatus(input: LogStatusInput): string | null {
  if (input.failed !== null) return input.failed
  if (input.noRepo) return 'This project has no git repository.'
  if (input.loading && input.rows === 0) return 'Reading the log…'
  if (input.rows > 0) return null

  // --- the walk stopped for a reason of its own, before any question about matching ---------
  if (input.stop === 'noSuchRef') return 'No branch of that name in this repository.'
  // Only reachable on a `resume` whose token this build cannot use — a rebuild between two
  // pages, or a token replayed from a detached window. A distinct sentence because the fix is
  // a refresh, and nothing else on this list is.
  if (input.stop === 'cursorLost') return 'The log moved on — refresh to start again.'
  if (input.stop === 'budget') {
    return 'Nothing found in the commits searched so far — keep looking below.'
  }
  if (input.stop === 'shallow') {
    return 'This clone is shallow and there is no history below its graft point.'
  }

  // --- the walk finished, so "nothing matched" is now a true statement ----------------------
  if (input.filtered) return 'No commit matches these filters.'
  if (input.path !== null) {
    return `Nothing in this repository's history touches ${input.path}.`
  }
  // `exhausted` with no rows and no filter is a repository with no commits at all — an unborn
  // HEAD, which `cide_git::log` reports as an empty page rather than an error because it is a
  // normal state for a freshly created project.
  return 'This repository has no commits yet.'
}

/**
 * The repositories in this page that could not resolve the chosen ref, by name.
 *
 * The merged case is the whole reason this exists. Under [`LogScope::Merged`] a branch filter
 * resolves *per repository*, and a root that lacks the branch reports [`LogStop::NoSuchRef`]
 * while the others answer normally — so the page has rows, [`logStatus`] returns `null`, and
 * the refusal would never reach the screen. `cide_git::log` deliberately does not refuse the
 * whole query there ("refusing would make the branch filter useless in exactly the monorepo it
 * exists for"), which puts the obligation to say so here.
 */
export function missingRef(page: CommitPage | null): string[] {
  if (page === null) return []
  return page.repos.filter((entry) => entry.stop === 'noSuchRef').map((entry) => entry.name)
}

/** [`missingRef`] as a sentence, or `null` when every repository resolved the ref. */
export function missingRefNote(page: CommitPage | null, filter: LogFilter): string | null {
  const names = missingRef(page)
  if (names.length === 0) return null
  // The names are joined rather than indexed because `noUncheckedIndexedAccess` makes every
  // `names[0]` a `string | undefined`, and a `?? ''` in a user-visible sentence is a way to
  // print an empty repository name and never notice.
  const where =
    names.length === 1
      ? names.join('')
      : `${names.slice(0, -1).join(', ')} and ${names.slice(-1).join('')}`
  const what = filter.branch.kind === 'branch' ? `“${filter.branch.name}”` : 'that revision'
  return `No ${what} in ${where}.`
}

/**
 * Why the graph is not drawn, in words, or `null` when the reason needs no explaining.
 *
 * In the model and not in the view for the reason the whole sentence table is here: this is the
 * sixth and seventh way this surface can be empty, and a reader comparing it against
 * [`logStatus`] can only see that no two of them say the same thing if they are in one file.
 *
 * `disabled` returns `null` on purpose. It is the one reason the *caller* chose — `graph: false`
 * in the query — and explaining a switch back to the person who flipped it is noise.
 */
export function graphOffReason(reason: GraphOff): string | null {
  switch (reason) {
    // A filtered list is not a DAG — its rows are a subsequence, and drawing edges between them
    // would assert a reachability nothing checked. IDEA greys the graph for the same reason; this
    // says so rather than leaving the column silently missing.
    case 'filtered':
      return 'The graph is hidden while a filter is on.'
    case 'merged':
      return 'The graph is hidden when several repositories are merged.'
    case 'fullHistory':
      return 'The graph is hidden in full-history mode.'
    case 'rerooted':
      return 'The graph restarts from here.'
    case 'disabled':
      return null
  }
}

/** The lane colour for a graph column, as a CSS custom property name. */
export function laneColor(index: number): string {
  // Eight, matching `cide_git::lanes::PALETTE`. The modulo is defensive: a forged or
  // future-versioned resume token could carry a larger index, and an undefined custom property
  // paints nothing at all, which reads as a missing line rather than a recycled colour.
  return `var(--lane-${index % 8})`
}

// --- the filter bar -------------------------------------------------------------------------

/**
 * How far the wire is allowed to lag behind the box.
 *
 * 200 ms is the interval a fast typist does not notice and a frontier walk over a hundred
 * thousand commits is not started four times per word. The gap is not empty — [`applyLocalText`]
 * narrows the page already on screen on the keystroke itself — so what the debounce actually
 * buys is *not starting walks nobody will read*, rather than the responsiveness, which the local
 * pass already provides.
 *
 * Exported so `LogTab.tsx` and `check-log.mjs` cannot hold two different numbers.
 */
export const FILTER_DEBOUNCE_MS = 200

/**
 * What the three controls in the filter bar hold.
 *
 * A value, not a query: the branch control is a [`LogRefs`] because that is the field it sets,
 * but the two text boxes are the raw strings the user typed, untrimmed. Trimming on the way in
 * would make the box refuse to accept a leading space that the user is about to type a word
 * after; trimming happens once, in [`filterOverrides`] and [`matchesText`], which are the two
 * places the string is actually *used*.
 */
export interface LogFilter {
  /** Where the walk starts. `{ kind: 'head' }` is the unfiltered default. */
  readonly branch: LogRefs
  /** Substring of the author's name **or** email, as `cide_git::log` matches it. */
  readonly author: string
  /** Substring of the message, or an oid prefix of at least four hex digits. */
  readonly text: string
}

/** The unfiltered state: HEAD, no author, no text. */
export const NO_FILTER: LogFilter = { branch: { kind: 'head' }, author: '', text: '' }

/** Whether anything is set — which is what decides whether *Clear filters* is offered. */
export function isFiltered(f: LogFilter): boolean {
  return f.branch.kind !== 'head' || f.author.trim() !== '' || f.text.trim() !== ''
}

/**
 * *Clear filters*.
 *
 * Returns the **same object** when there was nothing to clear, and that is the whole reason this
 * takes an argument instead of being a constant. `LogTab` compares the typed filter against the
 * applied one by reference to decide whether to start a request; a `clearFilters` that always
 * minted a fresh `NO_FILTER` would make a click on an already-clear bar start a walk that
 * returns the page already on screen.
 */
export function clearFilters(f: LogFilter): LogFilter {
  return isFiltered(f) ? NO_FILTER : f
}

/** Whether two filters ask the same question — the guard on the debounce. */
export function sameFilter(a: LogFilter, b: LogFilter): boolean {
  return a.author === b.author && a.text === b.text && sameBranch(a.branch, b.branch)
}

/** Whether two [`LogRefs`] name the same starting point. */
export function sameBranch(a: LogRefs, b: LogRefs): boolean {
  if (a.kind !== b.kind) return false
  if (a.kind === 'branch' && b.kind === 'branch') return a.name === b.name
  if (a.kind === 'rev' && b.kind === 'rev') return a.spec === b.spec
  return true
}

/**
 * ASCII-only case folding, matching `cide_git::log::contains_ci` fold for fold.
 *
 * Not `toLowerCase()`, which folds Unicode. The backend folds ASCII deliberately — full folding
 * needs the haystack decoded and normalised, which is a per-commit allocation — and a local pass
 * that folded *more* would keep a row on screen for one debounce interval and then drop it when
 * the wire disagreed. Matching the narrower rule is how the two passes stay one rule.
 */
function fold(value: string): string {
  return value.replace(/[A-Z]/g, (c) => c.toLowerCase())
}

/** git's own floor for an abbreviation, and the test `cide_git::log` applies to the needle. */
const OID_PREFIX_MIN = 4

function isOidPrefix(folded: string): boolean {
  return folded.length >= OID_PREFIX_MIN && /^[0-9a-f]+$/.test(folded)
}

/**
 * Whether a loaded row survives the text box — the instant half of the filter.
 *
 * **A case-insensitive substring, and deliberately not the fuzzy scorer.** `chrome/branchModel.ts`
 * records the reason and it applies harder here: fuzzy matching is useful over ten thousand paths
 * in a file picker and actively confusing over a list where `mn` matches `main`, `maintenance`
 * and `demand`. Nobody typing into a log filter means "score these commits"; they mean "the ones
 * with this ticket number in them".
 *
 * # Where this cannot mirror the backend, and which way each gap leans
 *
 * The wire rule is `contains_ci(message)` **or** a `starts_with` against the full oid once the
 * needle is four or more hex digits. Two gaps, each stated because each produces a visible
 * flicker for one debounce interval and neither is a bug to be fixed here:
 *
 * * **The body.** `CommitRow` carries `summary`, the first line, and not the message — a page of
 *   five hundred bodies is not something to put on the wire so a filter box can grep it. So the
 *   wire is *wider*: a commit whose ticket number is in its body arrives with the answer, having
 *   been hidden by the local pass until then. The local list only ever grows when the answer
 *   lands, which is the safe direction.
 * * **The author is deliberately NOT matched here**, and that is the whole shape of this rule.
 *   The wire's `text` greps the message and nothing else, so matching the author locally would
 *   make the local pass *wider* — and a row kept because a name matched would **disappear** a
 *   fifth of a second later when the answer lands. Every other gap leans the other way and only
 *   ever adds rows; one that removes them reads as a bug in a way that adding them never does.
 *   So the invariant is: **the local pass is always a subset of the wire's answer.** The author
 *   box is how you ask that question, and it is durable rather than a flicker.
 *
 * The oid is matched exactly as the backend matches it — a prefix of the **full** oid, gated at
 * four hex digits. The short oid is a prefix of the full one, so "what I can see on the row"
 * still works; a substring match on the short oid would have matched the middle of a hash, which
 * the wire never does.
 */
export function matchesText(row: CommitRow, text: string): boolean {
  const needle = fold(text.trim())
  if (needle === '') return true
  if (fold(row.summary).includes(needle)) return true
  return isOidPrefix(needle) && row.oid.startsWith(needle)
}

/**
 * The loaded rows, narrowed by the text box.
 *
 * Applied by `LogTab` **only while the typed text differs from the text the wire was last asked
 * for**. Once the answer for a needle has landed, the rows are already the backend's answer and
 * running this over them again would undo the body match the backend did and the row does not
 * carry — a commit found by its body would appear for a frame and then be filtered out by the
 * very pass that exists to make the box feel fast.
 */
export function applyLocalText(rows: readonly CommitRow[], text: string): CommitRow[] {
  if (text.trim() === '') return [...rows]
  return rows.filter((row) => matchesText(row, text))
}

/**
 * The fields of a [`LogQuery`] the filter bar sets.
 *
 * A `Partial<LogQuery>` handed to `logQuery(scope, over)` rather than a whole query, because
 * `LogQuery` has thirteen fields and `deny_unknown_fields` on the Rust side: a second place that
 * built one field by field is a second place to forget the fourteenth and get a deserialisation
 * failure instead of a default.
 *
 * An empty box is `null` and not `''`. `cide_git::log::needle` trims and treats empty as absent,
 * so the two are the same answer — but `null` is the one that says *no filter* on the wire, and
 * it is what [`LogResume`]'s query fingerprint hashes, so sending `''` from one call site and
 * `null` from another would make two identical questions produce two incompatible resume tokens.
 */
export function filterOverrides(f: LogFilter): Partial<LogQuery> {
  const author = f.author.trim()
  const text = f.text.trim()
  return {
    refs: f.branch,
    author: author === '' ? null : author,
    text: text === '' ? null : text,
  }
}

/**
 * The branch control's `<option>` value, and its inverse.
 *
 * A tagged string rather than the bare name, because a `<select>` speaks strings and a branch may
 * legitimately be called `head` or `all`. `b:head` and `head` are then different values and the
 * select cannot silently re-root onto HEAD because someone named a branch after it.
 */
export function branchValue(refs: LogRefs): string {
  switch (refs.kind) {
    case 'head':
      return 'head'
    case 'all':
      return 'all'
    case 'branch':
      return `b:${refs.name}`
    case 'rev':
      return `r:${refs.spec}`
  }
}

/** The inverse of [`branchValue`]. An unrecognised value falls back to HEAD, never throws. */
export function branchFromValue(value: string): LogRefs {
  if (value.startsWith('b:')) return { kind: 'branch', name: value.slice(2) }
  if (value.startsWith('r:')) return { kind: 'rev', spec: value.slice(2) }
  if (value === 'all') return { kind: 'all' }
  return { kind: 'head' }
}

// --- the repo strip, for a project with more than one root ------------------------------------

/**
 * The repository the log speaks for when it has to name one: the project's first root.
 *
 * The same choice, and the same sentence, as `chrome/branchModel.ts`: it is *"the only choice
 * that is stable across restarts"*. `cide_git::repo::discover` returns roots in the project's own
 * order with each root's submodules after it, so `repos[0]` is the directory the user opened.
 */
export function primaryRepo(repos: readonly RepoInfo[]): RepoInfo | null {
  return repos[0] ?? null
}

/**
 * Whether the rows need to say which repository they came from.
 *
 * One repository and every row would carry the same chip, which is a column of noise. More than
 * one and the log is a *merged* walk — which is also why there is no graph to look at: two
 * repositories share no DAG, so `cide_git::log` reports [`GraphOff::Merged`] and the chip is what
 * carries the structure instead. The strip appearing and the graph vanishing are two halves of
 * one fact, and they are decided by the same count.
 */
export function repoStripOn(repos: readonly RepoInfo[]): boolean {
  return repos.length > 1
}

/** A row's repository chip: what it says and what colour it is. */
export interface RepoChip {
  readonly name: string
  readonly color: string
}

/**
 * The chip for one row's repository, or `null` when the list does not name it.
 *
 * Coloured out of the lane palette by position, which is free precisely because a merged walk has
 * no graph to spend it on.
 *
 * `null` rather than a fallback, and rather than the raw id: a row from a repository the list has
 * never heard of means the list has not caught up — a `git init` in a bash pane between the two
 * calls — and a uuid in a 60px chip is worse than nothing at all. The next refresh fixes it, and
 * a chip that quietly disappears for one refresh is a smaller lie than a chip naming a uuid.
 */
export function repoChipFor(repos: readonly RepoInfo[], id: RepoId): RepoChip | null {
  const index = repos.findIndex((info) => info.id === id)
  const info = repos[index]
  if (info === undefined) return null
  return { name: info.name, color: laneColor(index) }
}

// --- refreshing when git moves underneath -----------------------------------------------------

/**
 * The directory every watched git file lives under, as a path *segment*.
 *
 * A segment and not a substring, because `src/dotgit/HEAD` and a file literally called
 * `foo.git` both contain the characters and neither is a git directory. Both separators are
 * split on: the wire carries whatever `PathBuf` printed, which is `/` on the only platform cide
 * has run on and `\` on the one it has not, and a predicate that silently answered "no git path
 * here" on Windows would turn this whole refresh off rather than fail.
 */
const GIT_DIR_SEGMENT = '.git'

/**
 * The files a burst may touch without any ref having moved.
 *
 * `index.lock` is here because it is half of every index write: git creates the lock, writes it,
 * renames it over `index`, and the watcher sees both names. A rule that admitted the lock but not
 * the file — or the other way round — would make the answer depend on which of the two events
 * survived coalescing.
 */
const GIT_INDEX_FILES = ['index', 'index.lock']

/** The last `/`- or `\`-separated component of a path. */
function baseName(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut === -1 ? path : path.slice(cut + 1)
}

/** Whether any component of this path is a `.git` directory. */
function underGitDir(path: string): boolean {
  return path.split(/[/\\]/).includes(GIT_DIR_SEGMENT)
}

/**
 * Whether a coalesced filesystem burst moved a **ref** — which is the question the log and the
 * blame gutter refresh on, and it is deliberately narrower than `FsChange.git`.
 *
 * # Why the flag alone is not the predicate
 *
 * `crates/cide-fs` sets `git` for a write to `HEAD`, the **index**, `ORIG_HEAD`, `FETCH_HEAD`,
 * `packed-refs` or anything under `refs/**` — one flag for "the branch readout and the git panel
 * should look again", which is the right granularity for a panel that walks the working tree and
 * the wrong one for a walk over history. `git add` rewrites `.git/index` and moves nothing;
 * so does `git status`, which a `watch -n1 git status` or an IDE plugin runs in a loop. Answering
 * every one of those with a fresh `git_log` — a frontier walk over a hundred thousand commits,
 * bounded by a scan budget but not free — is a background process the user did not ask for, and
 * it would throw away the page they are reading each time.
 *
 * So the paths are read, and the rule is: **any watched git path that is not the index means a
 * ref moved.** `HEAD` is a checkout, `refs/**` is a commit, a branch or a fetch, `packed-refs` is
 * every ref at once (the only signal a freshly cloned or `git gc`-ed repository gives), and
 * `ORIG_HEAD`/`FETCH_HEAD` are the rebase and the pull. Every one of those changes what a log
 * shows; only the index does not.
 *
 * # Three degradations, each chosen to fail towards refreshing
 *
 * * **`truncated`** means the burst was larger than the watcher holds and `paths` is a prefix —
 *   its documented contract is *"re-read what you care about"*. A truncated burst therefore
 *   counts as a ref move whatever survived in the list. Reading the prefix and concluding
 *   "index only" is exactly how a `git commit` inside a `cargo build` goes unnoticed.
 * * **No `.git` path in the list at all**, with the flag set, also counts. The flag is the
 *   watcher's own answer and this function's path parsing is a guess about spelling — a
 *   `$GIT_DIR` somewhere else, a linked worktree's common directory, a separator this rule did
 *   not think of. When the two disagree the flag wins, because a refresh nobody needed costs one
 *   walk and a refresh that never happens is the stale panel this whole predicate exists inside.
 * * **`git: false`** is the one hard `false`, whatever the paths say. A path under `.git` that
 *   the watcher did not classify as a git path is a path it is reporting for some other reason,
 *   and second-guessing it here would be a second implementation of `Filter::is_git_path`.
 *
 * Pure and exported so `check-log.mjs` can drive all four cases: a predicate this shape, written
 * inline in a subscription callback, is only checkable by having a repository and a stopwatch.
 */
export function gitRefsMoved(change: FsChange): boolean {
  if (!change.git) return false
  if (change.truncated) return true
  let sawGitPath = false
  for (const path of change.paths) {
    if (!underGitDir(path)) continue
    sawGitPath = true
    if (!GIT_INDEX_FILES.includes(baseName(path))) return true
  }
  return !sawGitPath
}

// --- revealing one commit ------------------------------------------------------------------

/**
 * A full oid, shortened for a sentence; anything else left alone.
 *
 * Seven characters, the same abbreviation `CommitRow::shortOid` and `BlameCommit::shortOid`
 * carry, so a note and the row it is about spell the commit the same way. Guarded on the shape
 * because the same helper labels a `LogRefs::Rev`, whose spec may be `v1.0..HEAD` — and
 * `"v1.0..H"` is not an abbreviation of that, it is a different revision expression.
 */
export function shortenOid(spec: string): string {
  return /^[0-9a-f]{40}$/.test(spec) ? spec.slice(0, 7) : spec
}

/** What the branch control calls a re-rooted walk. */
export function revLabel(spec: string): string {
  return `Revision ${shortenOid(spec)}`
}

/**
 * What became of a request to reveal one commit — see `requestLogReveal` in `LogTab.tsx`.
 *
 * # Why "not loaded" is a first-class outcome and not an edge
 *
 * The gesture this exists for is a click on a blame line, and a blame line is *usually* old: the
 * commit that last touched it can be years below the newest page, and it is guaranteed to be
 * below whatever a filter has narrowed the list to. So "the row is on screen, select it" is the
 * **uncommon** case. A reveal that only handled it would open the log at HEAD after a click on a
 * 2019 line and say nothing — the same "did anything happen?" state the notice in
 * `panes/EditorPane.tsx` was standing in for, arriving by a different route.
 *
 * `looking` is separate from `outside` for the same reason `logStatus` distinguishes loading from
 * empty: one is a reason to wait and the other is a sentence, and collapsing them makes a round
 * trip look like an answer.
 */
export type LogReveal =
  /** Nothing was asked for. */
  | { readonly kind: 'none' }
  /** Not in the loaded page; its detail is being fetched. */
  | { readonly kind: 'looking'; readonly oid: string }
  /** Found in the loaded page, selected, and scrolled to. */
  | { readonly kind: 'found'; readonly oid: string }
  /** Not in the loaded page, but it exists — its detail is in the pane. */
  | { readonly kind: 'outside'; readonly oid: string }
  /** The repository could not resolve it at all. */
  | { readonly kind: 'missing'; readonly oid: string }

/** No reveal outstanding. A shared constant, so an untouched tab re-renders with one identity. */
export const NO_REVEAL: LogReveal = { kind: 'none' }

/**
 * The sentence a reveal puts above the details pane, or `null` when the list speaks for itself.
 *
 * `found` says nothing on purpose: the row is selected and scrolled into view, which *is* the
 * answer, and a banner repeating it would be noise on the one path that worked. The other two
 * both have to say something, and they have to say different things — "this commit is not in
 * what you are looking at" and "this commit is not there at all" call for opposite next moves,
 * and the whole argument in [`logStatus`]'s header applies again here.
 */
export function revealNote(reveal: LogReveal): string | null {
  switch (reveal.kind) {
    case 'none':
    case 'found':
      return null
    case 'looking':
      return `Looking for commit ${shortenOid(reveal.oid)}…`
    case 'outside':
      return `Commit ${shortenOid(reveal.oid)} is outside the commits this list is showing.`
    case 'missing':
      return `Commit ${shortenOid(reveal.oid)} is not in this project’s history.`
  }
}

/**
 * Whether the note carries the *Clear filters and find it* button.
 *
 * Only `outside`, and that is the point of asking: the commit is known to exist, so re-rooting
 * the walk at it will certainly find it. Offering the same button for `missing` would be a
 * control that cannot work, and offering it for `looking` would be one that races the answer.
 */
export function revealFindable(reveal: LogReveal): boolean {
  return reveal.kind === 'outside'
}

/**
 * *Clear filters and find it*: every filter off, and the walk re-rooted at the commit itself.
 *
 * Clearing alone would not be enough and would look like it should be. The commit is below the
 * page, not hidden by a filter — an unfiltered page one still starts at HEAD and still stops
 * after `limit` rows, so clearing and re-fetching would land the user back on exactly the note
 * they pressed the button on. `LogRefs::Rev` is what actually answers the question: the walk
 * starts *at* that commit, so it is the first row of the answer and the pending reveal selects
 * it. `GraphOff::Rerooted` — "The graph restarts from here." — exists for this state, which is
 * the backend saying the same thing about the same gesture.
 */
export function findCommitFilter(oid: string): LogFilter {
  return { branch: { kind: 'rev', spec: oid }, author: '', text: '' }
}

// --- comparing two commits ---------------------------------------------------------------------

/**
 * Which commits the log is holding: one to read, or two to compare. (M20)
 *
 * # Why the pair lives here and not in `sidebar/clickSemantics.ts`
 *
 * That module answers a different question, and answering both there would make it two modules
 * sharing a filename. `clickSemantics` decides *what a gesture does to the row it landed on* —
 * select, toggle, open — for three trees whose rows are files, and its whole subject is the
 * conditional single-click-opens-a-diff rule that four bug reports produced. Nothing in it
 * carries state: every function is `(gesture, facts) => RowAction`, and a `RowAction` is three
 * booleans about one row.
 *
 * What is below is a *reducer over a selection*, and every rule in it needs two things that
 * module has never had: the previous selection and the list's own order. `selectRow` cannot be
 * expressed as a `RowAction` — "replace the older endpoint" is not select/toggle/open — and
 * `comparePair` is not about a gesture at all. Moving it there would mean either widening
 * `RowAction` for one caller or adding a second, unrelated shape beside it, and the first thing
 * the next reader would do is ask which of the two the file is about.
 *
 * The click *rule* the log does share — one click on a changed file retargets an open revision
 * diff, two open a kept tab — is already imported from there by `LogTab.tsx` rather than
 * restated, which is the part that would actually have drifted.
 *
 * # Two, and never three
 *
 * A diff has two sides. Everything below is written so that a third selection is impossible to
 * represent, rather than possible and then rejected somewhere later: `LogSelection` has exactly
 * two slots, so there is no state in which the pane would have to decide which two of three the
 * user meant.
 */
export interface LogSelection {
  /**
   * The anchor: the row a plain click landed on, and the row Shift extends from.
   *
   * Not "the newer side" — which of the two is newer is [`comparePair`]'s answer and is decided
   * by the log's order, never by which was clicked first. See its header.
   */
  readonly primary: string | null
  /** The second endpoint, or `null` for an ordinary one-commit selection. */
  readonly secondary: string | null
  /**
   * The user pressed **⇄ Swap**, so the pair is read against the log's order.
   *
   * A third field rather than "swap means exchange `primary` and `secondary`", and the reason is
   * that the two rules the brief asks for are otherwise contradictory. [`comparePair`] must order
   * the pair by the *log's* order so that clicking bottom-then-top does not invert the diff —
   * which means exchanging the two slots changes nothing at all, and the Swap control would be a
   * button that visibly does nothing on every pair that is actually on the page. Recording the
   * flip separately is what makes both true: the log decides the default orientation, and the
   * user can override it.
   *
   * Reset to `false` by every gesture in [`selectRow`] that changes *which* commits are selected.
   * A flip carried over onto a new pair would silently invert a diff the user never flipped, and
   * the header would still read the right way round because it is rendered from the same answer.
   */
  readonly swapped: boolean
}

/** Nothing selected. A shared constant, so an untouched tab re-renders with one identity. */
export const NO_SELECTION: LogSelection = { primary: null, secondary: null, swapped: false }

/** The modifier keys a click carried. `ctrl` is `Ctrl` or, on a Mac, `⌘` — the host folds them. */
export interface SelectMods {
  readonly ctrl: boolean
  readonly shift: boolean
}

/** Where an oid sits in the list, with "not on the page" sorting past every row that is. */
function rankOf(order: readonly string[], oid: string): number {
  const at = order.indexOf(oid)
  return at < 0 ? Number.POSITIVE_INFINITY : at
}

/**
 * A click on a row, with its modifiers, against the selection it landed on.
 *
 * `order` is the loaded rows' oids, **newest first** — the page's own order, which is what
 * decides every "older" below. It is passed rather than derived because this module has no page:
 * the same rule has to answer for a filtered list, whose order is a subsequence of the walk's.
 *
 * The four gestures:
 *
 * * **Plain click** replaces everything. It is the gesture that means "I am looking at this one",
 *   and a plain click that quietly kept a second row selected would leave the pane showing a
 *   range the user thought they had dismissed.
 * * **Ctrl** toggles the second endpoint: on an unselected row it adds one, on a selected row it
 *   removes that one and leaves the other as an ordinary single selection.
 * * **Ctrl on a third row replaces the *older* endpoint.** Not "ignore the click", and not "start
 *   again from this row". Ignoring it would make Ctrl+click read as a dead control on exactly the
 *   click where the user is trying to say something — two is what a diff has, so the third click
 *   has to mean *something* — and starting again would throw away the endpoint they have just
 *   spent two clicks establishing. Keeping the newer end and walking the older one down the list
 *   is what "compare against further and further back" looks like as a gesture, and it is
 *   reversible in one click.
 * * **Shift** extends from the anchor and keeps only the two ends, because a range of commits is
 *   not a thing this pane can show: `git_diff_revision_files` takes two sides. Selecting the
 *   whole span and then silently diffing its ends would be a selection that lies about what is
 *   being compared, so the intermediate rows are never selected in the first place.
 *
 * Shift is tested before Ctrl, so Ctrl+Shift extends. That is the arrangement in every list this
 * app's users also use; the alternative — treating the combination as neither — makes a slipped
 * finger do nothing at all.
 */
export function selectRow(
  sel: LogSelection,
  oid: string,
  mods: SelectMods,
  order: readonly string[],
): LogSelection {
  const only = (one: string | null): LogSelection => ({
    primary: one,
    secondary: null,
    swapped: false,
  })

  if (mods.shift) {
    // No anchor yet: Shift on a fresh list is a plain click. There is nothing to extend *from*,
    // and the alternative — refusing — leaves a list where the first Shift+click does nothing.
    if (sel.primary === null) return only(oid)
    // Extending onto the anchor itself is a range of one, which is a single selection.
    if (sel.primary === oid) return only(oid)
    return { primary: sel.primary, secondary: oid, swapped: false }
  }

  if (!mods.ctrl) return only(oid)

  if (sel.primary === oid) return only(sel.secondary)
  if (sel.secondary === oid) return only(sel.primary)
  if (sel.primary === null) return only(oid)
  if (sel.secondary === null) return { primary: sel.primary, secondary: oid, swapped: false }

  // The third click. Keep whichever endpoint is newer in the page's own order and replace the
  // other; `swapped` goes with the pair that is being replaced.
  const keep =
    rankOf(order, sel.primary) <= rankOf(order, sel.secondary) ? sel.primary : sel.secondary
  return { primary: keep, secondary: oid, swapped: false }
}

/** The two ends of a comparison, oldest-to-newest resolved. */
export interface ComparePair {
  /** The right-hand side — `RevSide` `new`. */
  readonly newer: string
  /** The left-hand side — `RevSide` `old`. */
  readonly older: string
}

/**
 * Which of the two endpoints is the newer one, or `null` when there is no pair.
 *
 * # The order is the log's, not the click's
 *
 * This is the whole reason the function takes `order`. A user comparing two commits clicks the
 * one they noticed first, which is as often the bottom of the two as the top; deriving "newer"
 * from the click sequence would show them a diff with every hunk inverted — additions as
 * deletions — and nothing on screen would say which way round it had gone. The list is already
 * sorted newest-first by the walk that produced it, so the answer is free and it is the answer
 * the user can see.
 *
 * # …unless they said otherwise
 *
 * [`LogSelection.swapped`] is the override, and it is applied last so that it flips whatever the
 * order decided rather than competing with it. That matters for the case the order genuinely
 * cannot answer: *Compare with…* can name a revision that is not on the loaded page at all — a
 * tag, `HEAD~200`, a branch tip below the frontier — and an oid the page does not hold sorts
 * **past every row that it does**, i.e. as the older side. That is a guess: `git rev-parse` gives
 * an oid and no date, and asking for one would be a second round trip to decide the orientation
 * of a diff the user is about to look at anyway. So the guess is the cheap one, the header says
 * which way round it went, and ⇄ Swap is one click.
 *
 * `null` for zero or one endpoint — there is no range yet — and also for a pair whose two ends
 * are the *same* commit, which *Compare with…* can produce by resolving a name that lands on the
 * selected row. A commit compared with itself has no files in it, and an empty list under a
 * "Comparing a1b2c3d … a1b2c3d" header is indistinguishable from a failed request.
 */
export function comparePair(sel: LogSelection, order: readonly string[]): ComparePair | null {
  const { primary, secondary } = sel
  if (primary === null || secondary === null) return null
  if (primary === secondary) return null
  const primaryIsNewer = rankOf(order, primary) <= rankOf(order, secondary)
  const newer = primaryIsNewer ? primary : secondary
  const older = primaryIsNewer ? secondary : primary
  return sel.swapped ? { newer: older, older: newer } : { newer, older }
}

/**
 * ⇄ Swap: read the same two commits the other way round.
 *
 * An involution — pressing it twice is where you started — because it toggles one boolean rather
 * than moving oids between slots. Moving them would also be an involution on the *selection* and
 * would not be one on the *diff*, since [`comparePair`] re-sorts by the page's order and would
 * put the pair straight back the way it was.
 *
 * A no-op on a one-row selection rather than an error: the control is only drawn beside a pair,
 * but the palette and a future binding both reach the same function, and a keystroke that throws
 * because nothing is selected is worse than one that does nothing.
 */
export function swapPair(sel: LogSelection): LogSelection {
  if (sel.primary === null || sel.secondary === null) return sel
  return { ...sel, swapped: !sel.swapped }
}

/**
 * Why two selected commits cannot be diffed: they are in different repositories.
 *
 * Reachable only under a merged scope, where one walk interleaves every root the project has and
 * two adjacent rows can come from two histories that share no object database. `git_diff_revision_files`
 * takes one `repo`, and there is no answer to give it — this is not a limitation of the command
 * but of git.
 *
 * A sentence and not a hidden menu line, because *Compare* being absent here would be
 * indistinguishable from the feature not existing: the user has two rows selected and the pane
 * says nothing. Both the disabled menu item and the details pane show this same string, passed
 * in from here, so the two cannot drift into two explanations of one refusal.
 */
export const CROSS_REPO_COMPARE =
  'These two commits are in different repositories, which share no history — there is nothing to diff.'

/**
 * What the pane says instead of a short list when `RevisionRange.truncated` is set.
 *
 * Naming the cap rather than saying "some files are missing": a range across a release really is
 * thousands of files, and a user who knows the number is 2 000 knows the list is capped rather
 * than broken and knows the next move is to narrow the range. A silently short list is the
 * failure this exists to prevent — it reads as a complete answer and there is nothing on screen
 * to contradict it.
 *
 * The number is `cide_git::revision::RANGE_FILE_CAP`, restated because the wire carries the flag
 * and not the cap; `check-log.mjs` reads the Rust constant and pins the two together, so a change
 * there fails here rather than leaving a sentence that names the wrong number.
 */
export const RANGE_FILE_CAP = 2000

/** [`RANGE_FILE_CAP`] as the sentence under a capped file list. */
export function rangeTruncatedNote(truncated: boolean): string | null {
  if (!truncated) return null
  return `Only the first ${RANGE_FILE_CAP.toLocaleString()} changed files are listed.`
}

/** `+12 −3` for one changed file. A real minus sign (U+2212), as everywhere else in this app. */
export function changeCounts(additions: number, deletions: number): string {
  return `+${additions} −${deletions}`
}

/**
 * The header over a two-sided file list: `Comparing <older> … <newer>`.
 *
 * Oldest on the left, which is the direction time runs in every range expression git accepts
 * (`old..new`) and the direction the diff itself is computed in. Reversing it to put the commit
 * the user clicked first on the left would make the header agree with the click and disagree
 * with the patch.
 *
 * Takes two **labels** and not a [`ComparePair`], because one of the two comparisons this pane
 * offers has no oid on the newer side at all: *Compare with working tree* is `RevSide::WorkingTree`
 * against a commit, and the files on disk have no hash. [`shortenOid`]'s guard is what makes one
 * function serve both — a forty-hex string is abbreviated and anything else is left exactly as it
 * was, which is the same reason it can label a `LogRefs::Rev` whose spec is `v1.0..HEAD`.
 */
export function compareTitle(older: string, newer: string): string {
  return `Comparing ${shortenOid(older)} … ${shortenOid(newer)}`
}

/** What the newer side is called when it is the files on disk rather than a commit. */
export const WORKING_TREE_LABEL = 'working tree'

/**
 * The sentence in the file list's place while `git_diff_revision_files` is in flight.
 *
 * Distinct from every empty state for the reason [`logStatus`]'s header gives at length: a range
 * across two release commits is thousands of deltas and takes long enough to see, and an empty
 * pane during it is indistinguishable from a comparison that found no differences — which is a
 * real answer two commits can have.
 */
export const RANGE_LOADING = 'Reading the comparison…'

/** What a comparison with no differing files says, so the empty pane is never the answer. */
export const RANGE_IDENTICAL = 'These two revisions have identical contents.'
