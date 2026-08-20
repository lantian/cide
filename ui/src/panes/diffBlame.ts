/**
 * Which commit wrote a diff row's line. The blame column's whole arithmetic. (M19)
 *
 * `GitDiffPane.tsx` draws the column and `editor/blameModel.ts` decides what a blame *says*;
 * this is the one question those two do not answer between them — **given a row of a unified
 * diff, which run of a blame covers it, and does it cover it at all**. The split is
 * `chrome/sidebarWidth.ts`'s and the rule is the one its header states: values in, values out.
 * Nothing here reads a clock, a DOM node or a store.
 *
 * # Deliberately import-free
 *
 * `scripts/check-diff-render.mjs` compiles this one file with the TypeScript in `node_modules`
 * and imports the output, exactly as `check-blame.mjs` does for `blameModel.ts`. A single
 * `import type … from '@/ipc/client'` would drag in the `@/*` path alias and behind it the whole
 * IPC surface, so [`BlameRunLike`], [`BlameCommitLike`] and [`LineOrigin`] are **structural
 * restatements** of the generated DTOs rather than those types — the same trade
 * `sidebarWidth.ts`'s `StoredSidebar` makes, and the same one `blameModel.ts` makes one
 * directory over.
 *
 * The restatement is not a second source of truth. `GitDiffPane.blameLookup` builds a
 * [`BlameLookup`] out of a real `BlameFile`, so a field renamed in
 * `crates/cide-ipc/src/history.rs` is a type error at that call site — and `check-diff-render.mjs`
 * additionally slices the field names out of `ui/src/ipc/generated.ts` and pins them, which is
 * the join that catches a rename before it shows up as `undefined` in a cell.
 *
 * # The one rule this file exists to state
 *
 * **The new side, and only the new side.** Everything below follows from it; see [`blameFor`].
 */

/**
 * The wire's `BlameRun`, restated. `commit` indexes [`BlameLookup.commits`].
 *
 * The runs are guaranteed **ascending, gapless and covering `[1, lines]`** —
 * `cide_git::blame::check_runs` asserts it on every route that produces a `BlameFile`, and
 * `blameModel.collapseRuns` re-checks it on arrival and refuses the whole answer if it is
 * broken. [`blameFor`] is therefore allowed to binary-search rather than scan, and a lookup that
 * reached it at all has already been through that gate.
 */
export interface BlameRunLike {
  /** First line of the run, **1-based**, like every other line number on this wire. */
  readonly start: number
  /** How many lines. Never zero. */
  readonly lines: number
  /** `null` for a line that is not committed — a local edit under a working-tree blame. */
  readonly commit: number | null
}

/**
 * The wire's `BlameCommit`, restated — the two fields a diff cell shows.
 *
 * Two and not eight, because this column is 22 characters wide and shows `a1b2c3d Ada Lovelace`.
 * The summary, the e-mail address and the author date are what the *editor*'s hover card is for,
 * and pulling them through here would mean restating fields nothing reads and pinning them in a
 * check that would then fail for a rename that could not possibly affect this column.
 */
export interface BlameCommitLike {
  /** The short oid, which is the form a reader would paste into `git show`. */
  readonly shortOid: string
  readonly author: string
}

/**
 * A blame, reduced to what a diff row needs to be looked up in it.
 *
 * `lines`, `runs` and `commits` are `BlameFile`'s own, restated. [`BlameLookup.buckets`] is the
 * one field that is **derived rather than carried**: the age tint needs a clock, this module may
 * not read one, and `blameFor`'s signature has nowhere to put a `now`. So the caller bakes the
 * ramp in — from `blameModel.collapseRuns`, so that a line's tint in this column and the same
 * line's tint in the editor's gutter cannot come from two different pieces of arithmetic.
 */
export interface BlameLookup {
  /**
   * Total lines of the blamed file.
   *
   * Read as a guard rather than for arithmetic, and it earns its place: the blame and the diff
   * are two round trips against a file that an agent may be writing between them, so a diff
   * whose new side runs past the end of the file that was blamed is a real state and not a bug.
   * Answering `null` past the end is how it stays a missing cell instead of a wrong one.
   */
  readonly lines: number
  readonly runs: readonly BlameRunLike[]
  readonly commits: readonly BlameCommitLike[]
  /**
   * The age bucket of each run, parallel to [`BlameLookup.runs`] — `blameModel.AGE_BUCKETS`'
   * six steps, or [`UNCOMMITTED_BUCKET`] for a run no commit owns.
   */
  readonly buckets: readonly number[]
}

/** The wire's `LineOrigin`, restated. See the header for why this is not an import. */
export type LineOrigin = 'context' | 'addition' | 'deletion'

/** The wire's `DiffSide`, restated. See [`blameRefusal`]. */
export type DiffSideLike = 'staged' | 'unstaged' | 'combined'

/**
 * What one cell of the column says.
 *
 * `uncommitted` is a flag of its own rather than `oid === ''`, because the two facts are
 * genuinely different and the view draws them differently: *this line is not in history* is an
 * answer, and *there is no cell here* — a deletion — is the absence of one. Collapsing them onto
 * an empty string would make an em dash and a blank the same state.
 */
export interface BlameCell {
  /** Short oid, or `''` when the line is not committed. */
  readonly oid: string
  /** Author name, or `''` when the line is not committed. */
  readonly author: string
  /** `blameModel.AGE_BUCKETS`' index, or [`UNCOMMITTED_BUCKET`]. */
  readonly bucket: number
  readonly uncommitted: boolean
}

/**
 * [`BlameCell.bucket`] for a line that is not committed. Never a valid age bucket.
 *
 * Restated from `blameModel.UNCOMMITTED_BUCKET` rather than imported, for the reason the header
 * gives, and `check-diff-render.mjs` compiles both files and pins the two numbers equal — the
 * stylesheet keys its "yours, not in history" tint on this exact value, so a drift would paint an
 * uncommitted line as though it were the newest committed one.
 */
export const UNCOMMITTED_BUCKET = -1

/**
 * The index of the run containing `line`, or `-1`.
 *
 * Binary search rather than a scan, and it is not premature: a twenty-thousand-line file blames
 * to a few thousand runs, `cell()` calls this once per drawn row, and a forty-hunk diff of that
 * file is four hundred rows. A scan makes that a million comparisons per paint for an answer
 * `log2(3000) ≈ 12` comparisons can give. The runs are sorted by `start` — see
 * [`BlameRunLike`] — which is what makes the search legal.
 */
function runAt(runs: readonly BlameRunLike[], line: number): number {
  let lo = 0
  let hi = runs.length - 1
  let found = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const run = runs[mid]
    // Unreachable — `mid` is always in range — but `noUncheckedIndexedAccess` is on and the
    // honest answer to "no run here" is the same as the answer to "no run covers this line".
    if (run === undefined) return -1
    if (run.start <= line) {
      found = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }
  return found
}

/**
 * Which commit wrote this row's line, or `null` when this row has no cell.
 *
 * # The new side, and only the new side
 *
 * A `DiffLineView` carries **both** line numbers, so annotating the old side is one more lookup
 * and it is deliberately refused. The next reader will see `oldLineno` sitting right there, so
 * this is the reason: the old side's lines belong to a *different document* — the index, or
 * `old_rev` — and attributing them needs a **second blame computed at that revision**. The pane
 * would then pay for two `git blame` walks of one file, one of them to fill a column nobody reads
 * while staging, on a surface whose job is to get a selection into the index. `README.md` records
 * the same refusal for the split view's left column and for the `@codemirror/merge` pane.
 *
 * # A deletion returns `null`
 *
 * It has no `newLineno`, and falling back to `oldLineno` would put a commit beside a line that
 * commit did not write — in a column whose entire claim is *this commit wrote this line*. The
 * origin is checked as well as the line number, so the refusal is by name rather than by the
 * accident of a null: a future wire that numbered deletions on both sides would otherwise start
 * quietly attributing them.
 *
 * # A context row and an addition both have `newLineno`
 *
 * …and both get the run covering it, with no special case. Against a working-tree blame an
 * addition is uncommitted, which the cell reports and the view draws as an em dash — matching the
 * editor gutter's `touched` set, which greys a line the session has changed rather than inventing
 * an attribution for it. Against a blame at `new_rev` — a historical diff — the same addition is
 * committed, by the very commit the tab is showing, and the cell names it. Same code, and the
 * difference is entirely in which blame the caller fetched.
 */
export function blameFor(
  lookup: BlameLookup | null,
  line: { origin: LineOrigin; newLineno: number | null },
): BlameCell | null {
  if (lookup === null) return null
  if (line.origin === 'deletion') return null
  const at = line.newLineno
  if (at === null || !Number.isInteger(at) || at < 1 || at > lookup.lines) return null
  const index = runAt(lookup.runs, at)
  const run = index === -1 ? undefined : lookup.runs[index]
  // Past the end of the run the search landed in. Unreachable against a checked cover, and kept
  // because this is the one place a hole in the runs would otherwise become a per-line falsehood
  // rather than a missing cell.
  if (run === undefined || at >= run.start + run.lines) return null
  const bucket = lookup.buckets[index] ?? UNCOMMITTED_BUCKET
  if (run.commit === null) return { oid: '', author: '', bucket, uncommitted: true }
  const commit = lookup.commits[run.commit]
  // A run indexing past the commit table. `collapseRuns` rejects the whole answer for this, so a
  // lookup that got here cannot have one; answering "no cell" rather than throwing keeps a
  // hand-built lookup from taking a pane down.
  if (commit === undefined) return null
  return { oid: commit.shortOid, author: commit.author, bucket, uncommitted: false }
}

/**
 * Why this diff side's new text cannot be blamed, or `null` when it can.
 *
 * # The staged side is the index, and nothing can blame the index
 *
 * `git_diff_file` answers `staged` as HEAD → **index**, so its `newLineno`s number the staged
 * blob. `cide_git::blame` can produce exactly three documents — a commit, the working file, or a
 * buffer the caller hands it (`blame.rs`'s *Which version of the file is blamed*) — and the index
 * is none of them. On a file whose worktree matches its index the two happen to coincide; on a
 * file with unstaged changes on top of staged ones they do not, and every line below the first
 * difference would name the wrong commit.
 *
 * That failure is invisible: every row still carries *a* short oid and *an* author, so the column
 * looks exactly as authoritative as a correct one. It is the same argument `collapseRuns` makes
 * for refusing a broken run set rather than repairing it — a plausible column that is wrong in a
 * way nobody can see is worse than a missing feature. So the side is refused outright rather than
 * blamed-when-clean, which would be a control that silently works or lies depending on state the
 * user is not looking at.
 *
 * The other two sides both end at the working tree (`unstaged` is index → worktree, `combined` is
 * HEAD → worktree), which is precisely the document a blame with no `newest` and no `contents`
 * produces.
 *
 * One function rather than two rules, because both the view (which writes the disabled toggle's
 * title) and the wiring (which withholds the handler and never fetches) have to agree about it,
 * and two copies of a sentence like this drift.
 */
export function blameRefusal(side: DiffSideLike): string | null {
  if (side !== 'staged') return null
  return (
    'The staged side’s new text is the index, and git can only blame a commit or the ' +
    'working file — with unstaged changes on top, those are different documents and every ' +
    'line below the first difference would name the wrong commit. Look at this file on ' +
    'Unstaged or All.'
  )
}
