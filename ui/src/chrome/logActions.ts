/**
 * Every decision and every sentence behind the commit log's *actions* — the half of the feature
 * a user actually reads. (M19)
 *
 * `crates/cide-git` performs a revert, a cherry-pick, a reset, an amend, a tag, a branch and a
 * detached checkout, and it is tested against the real `git`. None of that is what a person
 * meets. What they meet is a menu with seven lines in it and, for three of those lines, a dialog
 * asking whether they are sure — and *whether the dialog asked the right question* is not
 * something a Rust test can see. So the question lives here, in a module with no DOM and no
 * React in it, and `ui/scripts/check-log-actions.mjs` compiles it standalone and drives it.
 *
 * `chrome/branchModel.ts` is the template, down to the foot-note: **no runtime imports.** This
 * file goes one step further and has no imports at all, not even type-only ones. The wire shapes
 * are declared structurally below and pinned against `src/ipc/generated.ts` by the check, which
 * is the arrangement `branchModel.ts` arrived at for `FetchReport` — the same guarantee, reached
 * from the other side, and it means the check can compile one file with `types: []` and nothing
 * else on disk has to exist.
 *
 * # What gets a confirmation, and what deliberately does not
 *
 * **Revert and cherry-pick in Commit mode get no dialog at all.** That is a decision, not an
 * omission, and it is the most important one in this file:
 *
 * * they *add* a commit. Nothing is overwritten, nothing is unreachable afterwards, and the way
 *   back is `git reset --hard HEAD~1` — or the log's own reset, one row down;
 * * IDEA does not confirm them either, and it is the reference this feature is drawn from;
 * * and confirming a non-destructive act is exactly how a user learns to click through the ones
 *   that matter. A dialog that is always safe to dismiss trains the reflex that dismisses the
 *   `--hard` one. Rule 2 of `ConfirmDestructive` — *it only appears when something is at risk* —
 *   is the same argument written from the component's side.
 *
 * The safety for those two is elsewhere and it is real: `cide_git::replay` refuses on conflict
 * before writing anything (`GitError::ReplayWouldConflict`), refuses a merge without a mainline
 * (`GitError::MergeNeedsMainline`), and refuses a no-op (`GitError::EmptyReplay`). A typed
 * refusal that names the files is worth more than a dialog that says "are you sure" about an act
 * with a one-line undo — and [`explain`](./branchModel.ts) turns each of those into a sentence.
 *
 * **Working-tree mode is the one that gets one**, because it does not add a commit: it writes the
 * patch into the tree the user is standing in. That is the caller's dialog to raise, with
 * whatever the pre-check said would be overwritten.
 *
 * Three acts get one here:
 *
 * * **reset**, which is three different acts wearing one name — [`resetChoices`];
 * * **checking a commit out**, which detaches `HEAD` and may have to stash — [`detachConfirm`];
 * * **moving a tag that already exists**, which is a ref other people may already have fetched
 *   — [`forceTagConfirm`].
 *
 * And one *question* rather than a confirmation: [`mainlineChoices`], which turns
 * `GitError::MergeNeedsMainline` into a picker. A merge has more than one "before", so reverting
 * one without saying which side to keep is not a question with an answer; git refuses for the
 * same reason, and `-m` is how it makes you say.
 */

// --- the wire, structurally ----------------------------------------------------------------
//
// Declared rather than imported, for the reason the header gives. Each of these is a *subset* of
// the generated type — only the fields the sentences read — and that is the direction that
// works: a real `ResetPreview` is assignable to `ResetPreviewLike`, so the call sites hand over
// the wire value unchanged and TypeScript checks the join. `check-log-actions.mjs` reads the
// field names out of `src/ipc/generated.ts` so a rename in `crates/cide-ipc/src/git.rs` fails
// there rather than quietly producing `undefined` in a dialog.

/** A row of the log, as far as the menu and the confirmations need to look at one. */
export interface CommitRowLike {
  /** The full 40-hex oid — what every follow-up call passes back. */
  oid: string
  /** What is drawn, abbreviated by libgit2's own uniqueness rule. */
  shortOid: string
  summary: string
  /** Full oids of the parents, in git's order. More than one is a merge. */
  parents: readonly string[]
}

/** [`ResetPreview`], which is everything the reset dialog needs, computed before anything is written. */
export interface ResetPreviewLike {
  /** The branch that would move, or the short oid when detached. */
  head: string
  detached: boolean
  headOid: string
  targetOid: string
  targetSummary: string
  /** Reachable from `HEAD` but not from the target — what the reset drops. */
  commitsDropped: number
  /** Reachable from the target but not from `HEAD`. Non-zero means this is not a move backwards. */
  commitsGained: number
  /** Paths currently staged. `Mixed` loses this list's *staging*, not its contents. */
  staged: readonly string[]
  /** Paths dirty in the working tree. `Hard` loses this list's *contents*. */
  dirty: readonly string[]
  /** How many untracked files a `--hard` would **keep**. Stated positively; see the DTO. */
  untrackedKept: number
  /** Mirrors `RepoChanges::use_staging_area`. The whole reason Mixed has two sentences. */
  useStagingArea: boolean
}

/** How far back a reset moves the index and the working tree. Mirrors `ResetKind` on the wire. */
export type ResetKind = 'soft' | 'mixed' | 'hard'

/** Applying an existing commit's patch somewhere else. Mirrors `ReplayOp`. */
export type ReplayOpLike = 'revert' | 'cherryPick'

export interface ReplayOutcomeLike {
  op: ReplayOpLike
  /** The commit that was replayed, full oid. */
  source: string
  /** The commit that was created, full oid. **Empty** under working-tree mode, which creates none. */
  created: string
  summary: string
  files: number
}

export interface ResetOutcomeLike {
  kind: ResetKind
  headBefore: string
  headAfter: string
  commitsDropped: number
  filesDiscarded: number
  /** The shelf entry that was made, when the dialog's option was left ticked. */
  shelved: { name: string } | null
}

export interface DetachOutcomeLike {
  /** Short oid of the commit now checked out. */
  head: string
  summary: string
  /** The branch that was checked out before — the way back, and the point of the toast. */
  previous: string
  stashed: string | null
  restoreFailed: string | null
}

export interface TagOutcomeLike {
  name: string
  oid: string
  annotated: boolean
  /** An existing tag was force-updated rather than created. */
  moved: boolean
}

// --- the dialog, structurally ----------------------------------------------------------------

/**
 * One mode of an act that has several, in the shape `chrome/ConfirmDestructive.tsx`'s
 * [`ConfirmChoice`] takes.
 *
 * `danger` is required here and optional there, on purpose: every choice this module writes has
 * to have *decided*, so that `check-log-actions.mjs` can assert the red lands on exactly one of
 * the three. A choice that merely forgot the field would be indistinguishable from one that
 * chose `false`.
 */
export interface ConfirmChoiceLike {
  /** For a reset this is literally the `ResetKind` that goes on the wire. */
  id: string
  label: string
  body: string
  files: readonly string[]
  confirmLabel: string
  danger: boolean
}

/**
 * The **wording half** of a `ConfirmState` — everything this module can decide.
 *
 * Not the whole of one. `run`, `chosen`, `onChoose` and `option.onToggle` are callbacks and live
 * React state; a module that may not import anything cannot produce them, and a module that
 * *could* would be the wrong place to put them anyway. The caller spreads this and adds them,
 * exactly as `chrome/OutsideOpenGate.tsx` spreads `terminal/outsideOpen.ts`'s `OutsideAsk`.
 */
export interface ConfirmStateLike {
  title: string
  body: string
  files: readonly string[]
  confirmLabel: string
  /** Always set here. `−` means removal; these dialogs are mostly not removals. */
  mark: string
}

// --- what a row may do -------------------------------------------------------------------------

/** The per-commit actions, in the order the row menu lists them. */
export type CommitAction =
  | 'revert'
  | 'cherryPick'
  | 'reset'
  | 'amend'
  | 'tag'
  | 'branch'
  | 'detach'

/**
 * Every action, in menu order, with the one conditional line in its place.
 *
 * Ordered destructive-last within each group, and `detach` at the bottom because it is the only
 * one that changes where you are standing rather than what is in the repository.
 */
const ALL_ACTIONS: readonly CommitAction[] = [
  'revert',
  'cherryPick',
  'reset',
  'amend',
  'tag',
  'branch',
  'detach',
]

/**
 * The shortest abbreviation git will resolve by default, and the shortest this module will
 * accept as a prefix.
 *
 * Prefix matching is needed because `head` can arrive abbreviated — `BranchInfo.head` is a short
 * oid when `HEAD` is detached — while `CommitRow.oid` is always the full forty. Without a floor,
 * a `head` of `""` or `"a"` would match most of the log and put *Amend* on every row, which is
 * the failure that matters: amend is the one action that rewrites a commit.
 */
export const MIN_ABBREV = 7

/** Whether this row is the commit `HEAD` points at. */
export function isHead(row: CommitRowLike, head: string): boolean {
  if (head === '') return false
  if (row.oid === head || row.shortOid === head) return true
  return head.length >= MIN_ABBREV && row.oid.startsWith(head)
}

/**
 * Which actions `row` offers.
 *
 * **Amend only on `HEAD`; everything else on every row.** That asymmetry is git's, not a
 * simplification: `git commit --amend` replaces the tip and nothing else, and amending anything
 * older is an interactive rebase — a different act, with conflicts in it, and `cide_git` refuses
 * it by name (`GitError::NotHead`). The refusal exists as well as this filter, because a log page
 * can be seconds stale: another window, or a `git commit` typed into a terminal pane, moves
 * `HEAD` under a menu that is already open.
 *
 * Deriving the list rather than rendering seven items and disabling one is the rule the rest of
 * this app's menus follow — an item that is present and dead is a claim that something is
 * possible here. `chrome/branchModel.ts::actionsFor` is the same function for a branch row.
 */
export function actionsFor(row: CommitRowLike, head: string): CommitAction[] {
  const atHead = isHead(row, head)
  return ALL_ACTIONS.filter((action) => action !== 'amend' || atHead)
}

/** Whether this row is a merge, which is what makes a revert need a mainline. */
export function isMerge(row: CommitRowLike): boolean {
  return row.parents.length > 1
}

// --- the register ------------------------------------------------------------------------------
//
// The same handful of phrase-builders `branchModel.ts` keeps at the bottom of its file, for the
// same reason: one place that decides whether it is "1 commit" or "1 commits", so every sentence
// in this module agrees with every sentence in that one. Duplicated rather than imported — see
// the header — which is the trade `PAGE_ROWS` already makes over there.

/** How many hex characters an oid is shown as. Eight, like `PulledCommit::short_oid`. */
const OID_WIDTH = 8

/**
 * An oid as it is shown.
 *
 * Truncating rather than requiring the caller to pass the short form: half of these values are
 * full forty-hex oids (`ResetPreview::target_oid`, `ReplayOutcome::source`) and half are already
 * abbreviated (`DetachOutcome::head`), and a sentence that printed forty characters in the
 * middle of it would push everything after it off the line.
 */
function short(oid: string): string {
  return oid.length > OID_WIDTH ? oid.slice(0, OID_WIDTH) : oid
}

/** `1 commit` / `4 commits`. */
function commits(n: number): string {
  return `${n} ${n === 1 ? 'commit' : 'commits'}`
}

/** `1 file` / `12 files`. */
function files(n: number): string {
  return `${n} ${n === 1 ? 'file' : 'files'}`
}

/**
 * A commit summary in quotes, with a stand-in for the empty one.
 *
 * `“”` in the middle of a sentence reads as a rendering bug rather than as a commit whose
 * message is one blank line, and `chrome/branchModel.ts::fetchDetail` already settled on this
 * wording for the same case.
 */
function quoted(summary: string): string {
  return `“${summary === '' ? '(no summary)' : summary}”`
}

/** What the reset moves: the branch, or `HEAD` itself when there is no branch to move. */
function movingRef(p: ResetPreviewLike): string {
  return p.detached ? 'HEAD' : p.head
}

/**
 * `moves back 3 commits` / `moves forward 2 commits` / `moves to a1b2c3d`.
 *
 * Three phrasings because a reset is not always backwards. `ResetPreview::commits_gained` is
 * non-zero whenever the target is not an ancestor of `HEAD`, which happens every time somebody
 * resets *onto* a branch tip they are behind — a completely ordinary thing to do from a log —
 * and a dialog that could only say "0 commits would be undone" would be describing the wrong
 * act at the moment it matters most.
 */
function movement(p: ResetPreviewLike): string {
  if (p.commitsDropped > 0 && p.commitsGained === 0) return `moves back ${commits(p.commitsDropped)}`
  if (p.commitsGained > 0 && p.commitsDropped === 0) return `moves forward ${commits(p.commitsGained)}`
  return `moves to ${short(p.targetOid)}`
}

// --- reset ---------------------------------------------------------------------------------------

/**
 * `Reset main to a1b2c3d?`
 *
 * The branch is named, not "HEAD", because that is the thing the user will see move in the
 * status bar afterwards — and with several repositories in a project it is also the only word in
 * the dialog that says *which* one is about to move. Detached gets its own phrasing: there is no
 * branch, and saying `Reset a1b2c3d to 9f8e7d6c?` invites the reading that the *commit* is being
 * changed.
 */
export function resetTitle(p: ResetPreviewLike): string {
  return p.detached
    ? `Reset detached HEAD to ${short(p.targetOid)}?`
    : `Reset ${p.head} to ${short(p.targetOid)}?`
}

/**
 * `“fix the parser” — 3 commits would be undone.`
 *
 * The target's summary leads, because "a1b2c3d" is not something anyone can check and the commit
 * message is. Then the one fact that is true of all three modes: how far, and in which
 * direction. What each mode *costs* is the choice's body, not this — see [`resetChoices`].
 */
export function resetBody(p: ResetPreviewLike): string {
  const where = movingRef(p)
  const dropped = p.commitsDropped
  const gained = p.commitsGained
  const tail =
    dropped > 0 && gained > 0
      // Sideways: the target is on another line of history. Both numbers, because either one
      // alone describes half of what is about to happen.
      ? `${where} moves sideways — ${commits(dropped)} would be undone and ${commits(gained)} would arrive.`
      : gained > 0
        ? `${where} moves forward ${commits(gained)}.`
        : dropped > 0
          ? `${commits(dropped)} would be undone.`
          // Already there. Not a no-op — this is how a user un-stages or discards everything
          // without moving history, and it is worth saying that is what they are doing.
          : `${where} is already at ${short(p.targetOid)}, so only the index and the working tree would move.`
  return `${quoted(p.targetSummary)} — ${tail}`
}

/**
 * The three modes, least destructive first.
 *
 * The order is load-bearing twice over. `ConfirmDestructive` falls back to `choices[0]` when
 * nothing is selected yet, so first must mean safest; and a radio group is read top to bottom,
 * so the user meets *keep everything* before they meet *discard everything*.
 *
 * The ids are the wire's `ResetKind` values, so whatever the radio holds is what
 * `git_reset` is called with and there is no second table to drift.
 *
 * # Why Mixed has two sentences
 *
 * `--mixed` is the mode whose *cost* depends on a setting, and the gap is enormous:
 *
 * * in **staging-area mode** the index is the user's own work. They ran `git add -p`, they
 *   picked hunks, and a mixed reset throws that selection away — possibly an hour of it — with
 *   no undo, because git keeps no record of what was staged;
 * * in **changelist mode** the index is *derived*. `cide_git` rebuilds it from the changelists
 *   before every commit, so a mixed reset costs a recomputation the panel performs anyway, and
 *   nothing the user chose is lost.
 *
 * One sentence covering both would have to be vague enough to be true of the destructive case,
 * which means every changelist user reads a warning about work they cannot lose — and learns to
 * ignore it. `ResetPreview::use_staging_area` exists so this dialog can tell them apart.
 */
export function resetChoices(p: ResetPreviewLike): ConfirmChoiceLike[] {
  const where = movingRef(p)
  const staged = p.staged.length
  const dirty = p.dirty.length
  const kept = p.untrackedKept

  return [
    {
      id: 'soft',
      label: 'Soft',
      body:
        p.commitsDropped > 0
          ? `Keep everything. ${where} moves back ${commits(p.commitsDropped)} and their changes stay staged.`
          : `Keep everything. ${where} ${movement(p)} and the difference stays staged.`,
      // Nothing at all. This is the mode people reach for to rewrite a commit message, and the
      // empty list is the dialog saying so more convincingly than the sentence can.
      files: [],
      confirmLabel: 'Reset, keep everything',
      danger: false,
    },
    {
      id: 'mixed',
      label: 'Mixed',
      body: p.useStagingArea
        ? staged === 0
          ? `Keep your files, unstage everything. Nothing is staged right now, so this costs nothing beyond moving ${where}.`
          : `Keep your files, unstage everything. ${staged} staged ${staged === 1 ? 'file goes' : 'files go'} back to unstaged and the selection is lost.`
        : `Keep your files. ${where} ${movement(p)}; nothing you have selected in the panel is affected.`,
      /*
       * The staged paths, in **both** modes, and the second one is worth defending.
       *
       * In changelist mode nothing on this list is at risk, which sits awkwardly beside rule 1
       * ("it names what would be lost"). It is still the right list: these are the paths whose
       * index entries this reset rewrites, the sentence above says in so many words that the
       * selection survives, and the alternative — an empty list in one mode and a full one in
       * the other — makes the dialog *change shape* when a setting is flipped, which is the one
       * thing `ConfirmDestructive`'s header says these dialogs must never do.
       */
      files: p.staged,
      confirmLabel: p.useStagingArea ? 'Reset and unstage' : 'Reset, keep my files',
      danger: false,
    },
    {
      id: 'hard',
      label: 'Hard',
      body:
        (dirty === 0
          ? `Nothing in the working tree to discard, so ${where} ${movement(p)} and the tree is checked out clean at ${short(p.targetOid)}.`
          // Shouted, and it is the only shout in this app. `--hard` is the single gesture in the
          // log with no route back through git: the commits survive in the reflog for ninety
          // days, the file contents do not survive at all.
          : `DISCARD ${dirty} changed ${dirty === 1 ? 'file' : 'files'}. There is no undo for this in git.`)
        // Stated positively, and only when there is something to state. The most common fear at
        // this dialog is that new files are about to vanish; `git reset --hard` does not touch
        // them, and saying nothing leaves the user to guess — the guess is wrong. The count is
        // there because `ResetPreview::untracked_kept` exists to be said out loud.
        + (kept > 0 ? ` ${kept} untracked ${kept === 1 ? 'file is' : 'files are'} left alone.` : ''),
      files: p.dirty,
      confirmLabel: dirty === 0 ? 'Reset and check out' : `Discard ${files(dirty)}`,
      // The one red button in this dialog, and it stays red even when `dirty` is empty: the
      // colour belongs to the *mode*, which overwrites the working tree with no undo, not to
      // whatever happens to be dirty in the half-second the dialog was opened.
      danger: true,
    },
  ]
}

/** The label on the reset dialog's one checkbox. */
export const SHELVE_FIRST_LABEL = 'Shelve my changes first'

/**
 * Ticked, before the user touches anything.
 *
 * The recoverable answer should be the one a reflexive click produces — the same instinct that
 * puts focus on Cancel, one step further in. Somebody who has read the dialog and decided they
 * want the changes gone can untick it; somebody who has not gets their work back out of the
 * Shelf. The cost of the wrong default in one direction is a shelf entry to delete, and in the
 * other it is the work.
 */
export const SHELVE_FIRST_DEFAULT = true

/**
 * Whether the shelve-first checkbox is drawn at all.
 *
 * Only for `hard`, and only when there is something dirty to shelve. `--soft` and `--mixed`
 * leave the working tree exactly as it is, so offering to rescue it first would be a control
 * that never does anything — and an option the user has learned does nothing is one they stop
 * reading before the mode where it matters.
 */
export function shelveFirstOffered(p: ResetPreviewLike, chosen: string | null): boolean {
  return chosen === 'hard' && p.dirty.length > 0
}

// --- checking a commit out ------------------------------------------------------------------------

/**
 * The confirmation for checking a commit out of the log, which necessarily detaches `HEAD`.
 *
 * Nothing is destroyed here, so the glyph is `↗` and not `−` — the same override
 * `terminal/outsideOpen.ts` makes, and for the same reason: the mark is the one part of the
 * dialog that says *what kind of thing* is about to happen, and a removal dash over a list of
 * files that are about to be stashed and put back would be a sentence the dialog contradicts.
 *
 * Two bodies. With no blockers the dialog exists for a different reason from every other one in
 * this file: a detached `HEAD` is the state users most often reach by accident and least often
 * know how to leave, so the question being asked is "did you mean to leave your branch?" — and
 * the answer to "how do I get back" has to be in the dialog, not only in the toast afterwards.
 * With blockers it is the ordinary checkout refusal, and it names every file.
 */
export function detachConfirm(row: CommitRowLike, blockers: readonly string[]): ConfirmStateLike {
  const at = short(row.shortOid === '' ? row.oid : row.shortOid)
  return {
    title: `Check out ${at}?`,
    body:
      blockers.length === 0
        ? `${quoted(row.summary)} — this leaves your branch behind and puts you on a detached HEAD. Nothing is lost: the branch stays exactly where it is, and the toast will name it so you can get back with one click.`
        : `${quoted(row.summary)} — these files differ between your tree and ${at}, so checking out would overwrite them. cide stashes them first and puts them back afterwards; if the restore fails they are still in \`git stash list\`.`,
    files: blockers,
    confirmLabel: blockers.length === 0 ? `Check out ${at}` : 'Stash and check out',
    mark: 'arrow-up-right',
  }
}

// --- moving a tag ---------------------------------------------------------------------------------

/**
 * The confirmation for `git tag --force` — a tag that already exists, pointed somewhere else.
 *
 * No files, so the list is empty, and that is correct rather than a gap: nothing in the working
 * tree is at stake. What is at stake is a *ref other people may already have*, and git's own
 * refusal to move a tag without `--force` exists because a moved tag is the one kind of history
 * rewrite that spreads silently — `git fetch` will not update a tag it already has, so everyone
 * else keeps pointing at the old commit and nothing anywhere says the two disagree.
 *
 * The glyph is `→`, a move. Not `−`: the old tag is not deleted so much as relocated, and not
 * `↗`, which this file already uses for "you are going somewhere else".
 */
export function forceTagConfirm(name: string, existing: string, target: string): ConfirmStateLike {
  return {
    title: `Move tag ${name}?`,
    body: `${name} already points at ${short(existing)}. Forcing it moves the tag to ${short(target)}. Anyone who has already fetched ${name} keeps the old one until they fetch with --force, and nothing warns them — which is why git makes you ask twice.`,
    files: [],
    confirmLabel: `Move ${name}`,
    mark: 'arrow-right',
  }
}

// --- which side of a merge ---------------------------------------------------------------------------

/** The title over [`mainlineChoices`]. */
export const MAINLINE_TITLE = 'Which side of the merge should stay?'

/**
 * The sentence under it.
 *
 * It has to explain why it is being asked at all, because "mainline" is git jargon and the
 * question looks arbitrary to anyone who has not met `-m` before. A merge joins two histories,
 * so "undo this merge" has two possible meanings and git will not pick one for you.
 */
export function mainlineBody(oid: string): string {
  return `${short(oid)} is a merge, so it has more than one “before”. Reverting it means keeping one side and undoing the other, and only you know which is which.`
}

/**
 * A `GitError::MergeNeedsMainline` as a picker — one choice per parent — or `null` for anything
 * that is not one.
 *
 * The choices are numbered the way git numbers them: **1-based**, matching `git revert -m 1`, so
 * the id can go straight into `ReplayRequest::mainline` and a user who later reads the reflog
 * sees the same number they clicked. `danger` is `false` on every one of them — picking a
 * parent destroys nothing, it answers a question, and the replay that follows is the thing with
 * consequences.
 *
 * Each body is the parent's own summary and author, because that is what tells the two sides
 * apart; an oid pair does not. The suffix names the convention on top of that — parent 1 is the
 * branch the merge was made *on*, parent 2 is what was merged *in* — which is true of every
 * merge git creates and is the fact most people are missing when they meet this dialog.
 */
export function mainlineChoices(error: unknown): ConfirmChoiceLike[] | null {
  const parsed = wire(error)
  if (parsed === null || parsed.kind !== 'mergeNeedsMainline') return null
  const parents = field(parsed, 'parents')
  if (!Array.isArray(parents)) return null

  const choices: ConfirmChoiceLike[] = []
  for (const [index, parent] of parents.entries()) {
    if (typeof parent !== 'object' || parent === null) continue
    const it = parent as Record<string, unknown>
    const oid = typeof it['shortOid'] === 'string' ? it['shortOid'] : '?'
    const summary = typeof it['summary'] === 'string' ? it['summary'] : ''
    const author = typeof it['author'] === 'string' ? it['author'] : ''
    const n = index + 1
    const side =
      index === 0
        ? ' This is the side the merge was made on.'
        : index === 1
          ? ' This is the side that was merged in.'
          : ''
    choices.push({
      id: String(n),
      label: `Parent ${n} — ${oid}`,
      body: `${quoted(summary)}${author === '' ? '' : ` — ${author}`}.${side}`,
      // A mainline pick puts no file at risk. It is a question with two answers, drawn in the
      // dialog that already knows how to ask one.
      files: [],
      confirmLabel: `Use parent ${n}`,
      danger: false,
    })
  }
  // A `mergeNeedsMainline` with no parents in it is malformed — a picker with nothing to pick is
  // a dead dialog, and `null` sends the caller to `explain` instead, which at least says
  // something true.
  return choices.length === 0 ? null : choices
}

// --- what a completed action says ------------------------------------------------------------------
//
// Never empty, unlike `branchModel.ts::checkoutNote`. A checkout is its own feedback — the
// branch name in the status bar changed — but every action here happens in a log that looks
// identical afterwards, so silence is indistinguishable from a menu item that did nothing. Each
// note names a short oid, because the oid is the only thing a user can look up later.

/**
 * `Reverted a1b2c3d as e5f6a7b8 · 4 files`.
 *
 * The *created* commit is named as well as the source, because it is what the next gesture acts
 * on — the undo for a revert is a reset onto its parent, and that needs the new oid.
 *
 * Working-tree mode creates none (`ReplayOutcome::created` is empty by the convention
 * `FetchOutcome` set), and the note has to say so out loud: the whole difference between the two
 * modes is whether there is now a commit, and a user who assumes there is one and pushes will
 * push nothing.
 */
export function replayNote(o: ReplayOutcomeLike): string {
  const verb = o.op === 'revert' ? 'Reverted' : 'Cherry-picked'
  const src = short(o.source)
  if (o.created === '') {
    return `${verb} ${src} into the working tree · ${files(o.files)}, nothing committed`
  }
  return `${verb} ${src} as ${short(o.created)} · ${files(o.files)}`
}

/**
 * `Reset hard to a1b2c3d · 3 commits undone · 5 files discarded · shelved as “wip” · 9f8e7d6c is still in the reflog`.
 *
 * The last clause is the most valuable thing in this note and it is always there. `git reset`
 * has no undo in the porcelain, but the commits are not gone — `HEAD@{1}` is the old tip and it
 * survives for ninety days. A user who has just discovered they reset the wrong branch needs
 * that oid, and the moment they need it is after the toast has gone, which is why it is written
 * where it can be found again rather than only offered as a button.
 */
export function resetNote(o: ResetOutcomeLike): string {
  const parts = [`Reset ${o.kind} to ${short(o.headAfter)}`]
  if (o.commitsDropped > 0) parts.push(`${commits(o.commitsDropped)} undone`)
  if (o.filesDiscarded > 0) parts.push(`${files(o.filesDiscarded)} discarded`)
  if (o.shelved !== null) parts.push(`shelved as ${quoted(o.shelved.name)}`)
  parts.push(`${short(o.headBefore)} is still in the reflog`)
  return parts.join(' · ')
}

/**
 * `Detached at a1b2c3d — “fix the parser”. Back to main when you are done.`
 *
 * `previous` is named in every branch of this, because a detached `HEAD` is the state users
 * reach by accident and cannot leave without knowing where they came from — and by the time
 * they want to leave, the branch selector says `a1b2c3d detached` and nothing on screen
 * remembers `main`.
 *
 * A failed restore is returned verbatim and alone, exactly as `checkoutNote` does: git's own
 * message names the stash, and a sentence of ours wrapped around it would push the part that
 * matters off the end of a one-line note.
 */
export function detachNote(o: DetachOutcomeLike): string {
  if (o.restoreFailed !== null) return o.restoreFailed
  const at = `Detached at ${short(o.head)} — ${quoted(o.summary)}.`
  const back = `Back to ${o.previous} when you are done.`
  if (o.stashed !== null) return `${at} Your changes are in the stash — ${quoted(o.stashed)}. ${back}`
  return `${at} ${back}`
}

/**
 * `Tagged a1b2c3d as v1.2.0 · annotated`.
 *
 * *Moved* and *created* are different sentences because they are different acts with different
 * consequences for anyone who has fetched — see [`forceTagConfirm`] — and after the fact the
 * toast is the only record that a force happened at all.
 *
 * The kind is stated because it is invisible afterwards and it decides whether `git describe`
 * and most release tooling can see the tag; a user who wanted an annotated tag and got a
 * lightweight one finds out weeks later, in CI.
 */
export function tagNote(o: TagOutcomeLike): string {
  const kind = o.annotated ? 'annotated' : 'lightweight'
  return o.moved
    ? `Moved ${o.name} to ${short(o.oid)} · ${kind}`
    : `Tagged ${short(o.oid)} as ${o.name} · ${kind}`
}

// --- reading a wire error ---------------------------------------------------------------------------

/** A wire error, as far as this module needs to look at one. */
interface WireError {
  kind: string
  detail?: unknown
}

/*
 * Two twelve-line copies of `chrome/branchModel.ts`'s `wire` and `field`.
 *
 * Duplicated deliberately, and it is the same trade `PAGE_ROWS` makes over there: a value import
 * — even from another import-free module — is what stops `check-log-actions.mjs` compiling this
 * file on its own, and the check would then fail with a module-resolution error rather than
 * telling anyone what actually broke. The guarded reads are the part that must not drift, and
 * they are guarded here for the same reason they are guarded there: a rejection can be `null`,
 * and `(error as {kind?: string}).kind` throws on it, turning a recoverable refusal into an
 * unhandled rejection inside the catch block that was supposed to handle it.
 */
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

/*
 * Runtime values only, no imports of any kind: `check-log-actions.mjs` compiles this file
 * alongside `branchModel.ts` and loads the emitted `.js` in node. Adding a value import — a
 * store, `@/ipc/client`, even `./branchModel` — breaks that, and the check would start failing
 * with a resolution error rather than a sentence. The same note is at the foot of
 * `chrome/branchModel.ts`, `terminal/outsideOpen.ts` and `chrome/notices.ts`.
 */
