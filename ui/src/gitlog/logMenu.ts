/**
 * The commit row's menu, and the three requests it builds — as values, with no React in them.
 *
 * Its own module, and the reason is the same one `chrome/menuModel.ts` and `menus/model.ts` give
 * one folder over: there is no browser and no jsdom in this harness, so a rule that lives inside
 * a `useContextMenu({ items: … })` closure is a rule nothing can compile and nothing can assert.
 * Which item appears on which row, which one is disabled and *why*, and what the reset dialog's
 * answer turns into on the wire are all decisions, and every one of them is wrong in a way that
 * still renders a plausible menu.
 *
 * This began life as a `#region` inside `LogTab.tsx` that `check-log.mjs` sliced out as **text**
 * and compiled in a temp directory. That worked and was honest — it compiled the real source —
 * but the slicing was the fragile part: a moved marker, a stray brace, or a value import added
 * without thinking would each have broken the check in a way that looked like a test failure
 * rather than a build one. A file is what the rest of this project uses, and the check now just
 * names it.
 *
 * **DOM-free, and it stays that way.** The wire and menu shapes below are structural *subsets*
 * of the generated ones rather than imports of them, which is the direction that works:
 * `commitMenu`'s result is handed straight to `useContextMenu` and `resetRequestFor`'s to
 * `commitActions.reset`, so `tsc --noEmit` over the whole app checks the join at the call site.
 * `chrome/logActions.ts` reached the same arrangement from the other side and its header has the
 * long version.
 *
 * **`noClipboard` is a parameter, not an import.** It is `chrome/menuModel.ts`'s `NO_CLIPBOARD`,
 * passed in by the caller. The check reads that same constant out of that file's source and
 * asserts the item carries it — a real join. A sentence this module defined itself, and the
 * check then asserted on, would prove nothing.
 *
 * Every user-visible sentence is `chrome/logActions.ts`'s property, and `actionsFor` is the
 * single answer to "which of the seven does this row offer". Nothing here re-spells either.
 */
import {
  actionsFor,
  type CommitAction,
  type CommitRowLike,
  type ResetKind,
} from '@/chrome/logActions'

/**
 * One line of the row menu, in the shape `menus/model.ts`'s [`MenuItem`] takes.
 *
 * There is deliberately no `disabled: boolean` here either. The menu system dropped it because
 * a greyed-out control with no explanation is the most common way this app has wasted a user's
 * time, and the rule that makes it stick is the one `menus/model.ts` implements:
 * `run: enabled ? (entry.run ?? null) : null`. An item carrying **both** a `disabledReason` and
 * a `run` therefore looks wired and is dead — which is why `check-log.mjs` asserts that no item
 * this function produces carries both, on either kind of row.
 */
export interface CommitMenuItem {
  readonly kind?: 'item' | undefined
  readonly id: string
  readonly label: string
  /** Present ⇒ the item is disabled, and this is the sentence shown in place of a keychip. */
  readonly disabledReason?: string | undefined
  readonly danger?: boolean | undefined
  readonly run?: (() => void) | undefined
}

export interface CommitMenuSeparator {
  readonly kind: 'separator'
}

export type CommitMenuEntry = CommitMenuItem | CommitMenuSeparator

/**
 * What each line does, supplied by the host.
 *
 * Every one of them is `() => void` and none of them is async: the flows below round-trip,
 * branch on a refusal and sometimes raise a second dialog, and a menu item that returned a
 * promise would put the deciding of *which* dialog inside the menu.
 */
export interface CommitMenuActions {
  revert: () => void
  cherryPick: () => void
  reset: () => void
  tag: () => void
  branch: () => void
  detach: () => void
  /** Compare the two rows the page already has selected. See [`CommitMenuInput.pair`]. */
  compare: () => void
  /** ⇄ — read the same two commits the other way round. `logModel::swapPair`. */
  swapSides: () => void
  /** This commit against the files on disk: `new: WorkingTree`, `old: Commit`. */
  compareWorkingTree: () => void
  /** Raise the *Compare with…* overlay, which resolves a typed name to an oid before using it. */
  compareWith: () => void
  /**
   * Absent when the webview exposes no clipboard, which WebKitGTK does not always.
   *
   * Resolved by the host — `navigator` is a DOM global and this region may not touch one — and
   * absent means *Copy revision number* is drawn disabled with [`CommitMenuInput.noClipboard`]
   * on it. Never drawn and dead: that is the same rule `chrome/menuModel.ts`'s *Copy path*
   * follows, and it is the whole reason `TabActions.copy` is optional over there.
   */
  copy?: ((oid: string) => void) | undefined
  /**
   * Load this commit's message into the Git panel's commit box with *Amend* ticked.
   *
   * Optional for the same reason [`copy`] is, and with the same consequence: absent means the
   * item is drawn disabled carrying [`AMEND_ELSEWHERE`], never absent-and-dead. What makes it
   * absent here is a host that has no sidebar to reveal — a detached-pane window — which is the
   * one place the flow genuinely cannot complete.
   *
   * `() => void` like the rest, and that costs something worth naming: the host must round-trip
   * `git_commit_detail` for the message before it can park anything, so the click is answered
   * asynchronously and a failed fetch reports on the host's own status line. Making this arm
   * `async` would not change that — it would only move the deciding of which failure to show
   * into the menu, which is what the interface header refuses for all ten of these.
   */
  amend?: (() => void) | undefined
}

export interface CommitMenuInput {
  readonly row: CommitRowLike
  /**
   * The oid `HEAD` points at **in this row's repository**, or `''` when it is not known.
   *
   * Per repository, because a merged walk interleaves several roots and each has its own tip;
   * one `head` for the page would put *Amend* on a row that is the tip of some *other*
   * repository. `''` is the honest answer when the tip is not on the loaded page at all — a
   * filtered walk, or a branch filter — and `logActions::isHead` reads it as "no".
   */
  readonly head: string
  /** `chrome/menuModel.ts::NO_CLIPBOARD`. A parameter — see the region header. */
  readonly noClipboard: string
  /**
   * The two-commit selection the page is holding, or `null` for an ordinary one-row selection.
   *
   * Already resolved into newer/older by `logModel::comparePair`, because *which* of the two is
   * the newer side is decided by the log's own order and this module has no page to read it off.
   * Handing the menu two unordered oids and letting it sort them would put that decision in two
   * places, and the second one would be the one with no `order` to sort by.
   */
  readonly pair: CommitMenuPair | null
  /**
   * `logModel::CROSS_REPO_COMPARE`. A parameter, exactly as `noClipboard` is, and for the same
   * reason the header gives: the details pane shows this same sentence when it declines to fetch,
   * and a string this module defined itself would be a second wording of one refusal.
   */
  readonly crossRepo: string
  readonly on: CommitMenuActions
}

/** The pair, plus the one fact about it that decides whether it can be diffed at all. */
export interface CommitMenuPair {
  /** The right-hand side: `RevSide` `new`. */
  readonly newer: string
  /** The left-hand side: `RevSide` `old`. */
  readonly older: string
  /**
   * Both endpoints came out of the same repository.
   *
   * False only under a merged scope, where one walk interleaves every root the project has. It is
   * the host's answer and not something derivable from two oids — `CommitRow.repo` is what says
   * so, and a commit that came from *Compare with…* carries the repository the overlay resolved
   * it against.
   */
  readonly sameRepo: boolean
}

/**
 * Why *Amend…* is disabled in a host that supplies no [`CommitMenuActions.amend`].
 *
 * The act is wired end to end — `git_commit` takes a `CommitRequest.amendOf` and
 * `cide_git::commit::require_amend_head` refuses anything but the tip with `GitError::NotHead`
 * — but the *control* is the Git panel's commit box, which owns the message, the ticked paths
 * and the Amend checkbox. Clicking here does not amend; it fetches the commit's message, parks
 * it through `chrome/panelRequests`, and reveals the panel so the box adopts it. A window with
 * no sidebar has nothing to reveal and nothing to adopt the request, which is the case this
 * string covers.
 *
 * Listed rather than hidden, because `actionsFor` puts it on the tip row and nowhere else — its
 * presence is the answer to "can this commit be amended at all", which is a different question
 * from "can it be amended from this window".
 */
export const AMEND_ELSEWHERE = 'Amend from the Git panel — tick Amend beside the commit message'

/**
 * The seven actions, the four comparisons, and *Copy revision number*, in menu order.
 *
 * The mutating order is `logActions::ALL_ACTIONS` and the grouping is what that order means:
 * apply this commit's patch somewhere else, then rewrite what is here, then point a new ref at
 * it, then go and stand on it, then copy its name. `menus/model.ts::resolveMenu` collapses runs
 * of separators and drops the leading and trailing one, so a group that turns out empty —
 * *Amend* on any row but the tip, the compare group on a one-row selection — costs nothing and
 * needs no condition here.
 *
 * # Why the compare group is first
 *
 * It is the only group in this menu that changes nothing. Everything below it writes to the
 * repository, and two of them can destroy work; the first item under the pointer when a menu
 * opens is the one a slipped click lands on, and *Revert* was that item until this group existed.
 * The order is also the order of frequency — reading a diff is what a log is for — so the cheap
 * reading is that nothing was traded to get the safety.
 *
 * The group that lost: putting the comparisons at the *bottom*, beside *Copy revision number*, on
 * the grounds that both are "read-only". That reading is wrong about what the bottom of a menu is
 * for — it is where the incidental things go — and it would have buried the feature under six
 * lines the user is trying not to press.
 *
 * `danger` is on *Reset here…* and on nothing else. Not on *Check out this commit*, even though
 * it stashes: `logActions::detachConfirm` marks that dialog `↗` rather than `−` precisely
 * because nothing is destroyed, and spending the red on it is how the red stops meaning
 * anything on the item that opens a `--hard`.
 */
export function commitMenu(input: CommitMenuInput): CommitMenuEntry[] {
  const { row, head, noClipboard, pair, crossRepo, on } = input
  const offered = actionsFor(row, head)
  const has = (action: CommitAction): boolean => offered.includes(action)
  /*
   * Hoisted before the entries, not read as `on.copy` inside the closure below.
   *
   * The same narrowing trap `chrome/ConfirmDestructive.tsx` writes down: TypeScript will not
   * carry a narrowing of a property across a function boundary, because the object could be
   * reassigned as far as the checker knows. A local `const` narrows once and stays narrowed.
   */
  const copy = on.copy

  const entries: CommitMenuEntry[] = []
  /*
   * *Compare* and *Swap sides* exist only while two rows are selected.
   *
   * Derived and not disabled, which is the same rule *Amend* follows one group down: an item that
   * is present and dead is a claim that something is possible here, and "compare" with one row
   * selected is not a thing with a missing precondition — it is a different gesture (Ctrl+click a
   * second row) that the two items below already offer a route to.
   *
   * *Compare* is listed even though selecting the second row has **already** put the range in the
   * details pane, and that redundancy is deliberate. Ctrl+click on a list is a chord nobody
   * discovers by accident; the menu is where a user looks for "what can I do with these two", and
   * a menu that answered "nothing" there would be telling them the range they are looking at does
   * not exist. It re-asks the same question, which is also the way to refresh a range after a
   * `git commit` in a terminal pane moved something under it.
   */
  if (pair !== null) {
    entries.push({
      id: 'compare',
      label: 'Compare',
      ...(pair.sameRepo ? { run: on.compare } : { disabledReason: crossRepo }),
    })
    /*
     * Swap stays live even for a cross-repository pair, where *Compare* above is not.
     *
     * It is state and not an act: it flips the header, costs nothing, and is undone by pressing
     * it again. Disabling it would leave a user whose two rows came from two roots looking at a
     * header they cannot even reorder, which reads as a second failure rather than as the same
     * one.
     */
    entries.push({ id: 'compareSwap', label: 'Swap sides', run: on.swapSides })
  }
  /*
   * *Compare with working tree* is on every row and is never disabled.
   *
   * `RevSide::WorkingTree` is legal as `new` against any commit — that is the one legal pairing
   * `cide_git::revision` states in its own header — so there is no state of the page in which
   * this refuses. A row from a bare repository would be the exception, and a bare repository has
   * no working tree to have opened the project from in the first place.
   */
  entries.push({
    id: 'compareWorkingTree',
    label: 'Compare with working tree',
    run: on.compareWorkingTree,
  })
  /*
   * The ellipsis is the promise the rest of this menu keeps: something will ask before anything
   * happens. What it asks for is a revision, and the answer goes through `git_resolve_rev` before
   * it is used anywhere — a name is not an oid, and `main` means a different tree tomorrow.
   */
  entries.push({ id: 'compareWith', label: 'Compare with…', run: on.compareWith })
  entries.push({ kind: 'separator' })
  if (has('revert')) entries.push({ id: 'revert', label: 'Revert', run: on.revert })
  if (has('cherryPick')) entries.push({ id: 'cherryPick', label: 'Cherry-pick', run: on.cherryPick })
  entries.push({ kind: 'separator' })
  if (has('reset')) entries.push({ id: 'reset', label: 'Reset here…', danger: true, run: on.reset })
  if (has('amend')) {
    entries.push(
      on.amend === undefined
        ? { id: 'amend', label: 'Amend…', disabledReason: AMEND_ELSEWHERE }
        : { id: 'amend', label: 'Amend…', run: on.amend },
    )
  }
  entries.push({ kind: 'separator' })
  if (has('tag')) entries.push({ id: 'tag', label: 'Tag…', run: on.tag })
  if (has('branch')) entries.push({ id: 'branch', label: 'New branch from here…', run: on.branch })
  entries.push({ kind: 'separator' })
  if (has('detach')) {
    entries.push({ id: 'detach', label: 'Check out this commit (detached)', run: on.detach })
  }
  entries.push({ kind: 'separator' })
  entries.push({
    id: 'copyOid',
    /*
     * IDEA's wording, and the item copies the **full forty** while the row shows seven of them.
     * That gap is the whole point of the line — nobody can read the rest off the screen — and it
     * is why the notice afterwards prints what went on the clipboard rather than saying "copied".
     */
    label: 'Copy revision number',
    ...(copy === undefined ? { disabledReason: noClipboard } : { run: () => copy(row.oid) }),
  })
  return entries
}

/**
 * Which choice a `ConfirmDestructive` with a radio group is actually on.
 *
 * A copy of that component's own fallback — `choices.find(c => c.id === state.chosen) ??
 * choices[0]` — and it has to be, because the component resolves it internally and the caller
 * is the one that has to send the answer over the wire. `check-log.mjs` pins the two together
 * against `ConfirmDestructive.tsx`'s source, so the day the fallback there changes is the day
 * this fails rather than the day a reset silently performs the wrong mode.
 *
 * The fallback is what makes `logActions::resetChoices` ordering load-bearing: least
 * destructive first, so an unanswered dialog resolves to `soft`.
 */
export function pickedId(
  choices: readonly { readonly id: string }[],
  chosen: string | null,
): string | null {
  return choices.find((choice) => choice.id === chosen)?.id ?? choices[0]?.id ?? null
}

/** How far back the reset moves the index and the tree. Anything unrecognised is the safe one. */
export function resetKindOf(id: string | null): ResetKind {
  // Not a cast of the radio's value. The ids *are* the wire's `ResetKind` — that is why
  // `resetChoices` uses them — but a value that came out of a dialog and goes into a command
  // that can discard a working tree is worth one comparison. The unknown case resolves to
  // `soft`, which changes no file, rather than to whatever the string happened to be.
  return id === 'hard' ? 'hard' : id === 'mixed' ? 'mixed' : 'soft'
}

/**
 * Which parent of a merge to keep, out of a `mainlineChoices` id.
 *
 * `null` for anything that is not a whole number ≥ 1, and that is not defensive padding:
 * `Number.parseInt('')` is `NaN`, `JSON.stringify(NaN)` is `null`, and a `mainline: null` on the
 * wire is exactly what raises `MergeNeedsMainline` again — so a malformed id would put the same
 * picker back on screen for ever. Refusing here means the retry simply does not happen.
 */
export function mainlineOf(id: string | null): number | null {
  const n = Number.parseInt(id ?? '', 10)
  return Number.isInteger(n) && n >= 1 ? n : null
}

/** The shape [`ReplayRequest`] has on the wire, structurally. */
export interface ReplayRequestLike {
  commit: string
  mode: 'commit' | 'workingTree'
  mainline: number | null
}

/**
 * A revert or a cherry-pick, always in **Commit** mode.
 *
 * The mode is fixed here rather than offered, and that is the same decision `logActions`'s
 * header defends at length from the other side: Commit mode *adds* a commit, so nothing is
 * overwritten, the way back is one reset, and the menu therefore raises no dialog at all.
 * Working-tree mode writes the patch into the tree the user is standing in, which is the mode
 * that would need one — and there is no gesture for it in this menu, so there is no dialog.
 *
 * `mainline` is `null` on the first attempt for every row, merge or not. Guessing 1 would
 * silently revert an entire feature branch or none of it with no way to tell which until the
 * diff is read; `MergeNeedsMainline` carries the parents so the second attempt can ask.
 */
export function replayRequestFor(commit: string, mainline: number | null): ReplayRequestLike {
  return { commit, mode: 'commit', mainline }
}

/** The shape [`ResetRequest`] has on the wire, structurally. */
export interface ResetRequestLike {
  target: string
  kind: ResetKind
  shelveFirst: string | null
  force: boolean
}

/** How many hex characters the shelf name carries. Eight, as everywhere else in this feature. */
const SHELF_OID_WIDTH = 8

/**
 * What the rescued working tree is called on the Shelf.
 *
 * Named after what it is *before*, not after the commit it is being reset to, because that is
 * how the user will look for it: they will remember resetting, and the shelf list is sorted by
 * time. `ResetRequest.shelveFirst` is an `Option<String>` rather than a bool plus a name
 * precisely so there is no way to ask for a shelf without naming it.
 */
function shelfNameFor(target: string): string {
  return `Before reset to ${target.slice(0, SHELF_OID_WIDTH)}`
}

/**
 * The dialog's answer, as the wire takes it.
 *
 * `force: false`, always. It is the *second* click past a guard that has already listed what is
 * at stake, and this dialog is the first — `ResetRequest.force` says in as many words that it is
 * never defaulted to true anywhere.
 */
export function resetRequestFor(
  target: string,
  kind: ResetKind,
  shelve: boolean,
): ResetRequestLike {
  return { target, kind, shelveFirst: shelve ? shelfNameFor(target) : null, force: false }
}

/** The shape [`TagRequest`] has on the wire, structurally. */
export interface TagRequestLike {
  name: string
  target: string
  message: string | null
  force: boolean
}

/**
 * A tag, annotated exactly when the message box has something in it.
 *
 * Which field is present *is* the choice — there is no `annotated: bool` that could disagree
 * with the message — so an empty or blank box has to become `null` and not `''`. The difference
 * is not cosmetic: `git describe` and most release tooling ignore lightweight tags, and a user
 * who wanted an annotated one and got a lightweight one finds out weeks later, in CI.
 */
export function tagRequestFor(
  name: string,
  target: string,
  message: string,
  force: boolean,
): TagRequestLike {
  const body = message.trim()
  return { name: name.trim(), target, message: body === '' ? null : body, force }
}

/**
 * One string out of a tagged wire error, or `null` when this is not that error.
 *
 * Needed for exactly one thing — `TagExists`'s `oid`, which is where the existing tag points and
 * therefore the whole content of the question `forceTagConfirm` asks — and written guarded for
 * the reason `chrome/branchModel.ts::wire` gives: a rejection can be `null`, and
 * `(error as {kind?: string}).kind` throws on it, turning a refusal the user could have answered
 * into an unhandled rejection inside the catch block that was supposed to handle it.
 */
export function errorField(error: unknown, kind: string, key: string): string | null {
  if (typeof error !== 'object' || error === null) return null
  if ((error as { kind?: unknown }).kind !== kind) return null
  const detail = (error as { detail?: unknown }).detail
  if (typeof detail !== 'object' || detail === null) return null
  const value = (detail as Record<string, unknown>)[key]
  return typeof value === 'string' ? value : null
}
