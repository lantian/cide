/**
 * What a blame gutter *says*: the column's arithmetic, with no CodeMirror and no IPC in it. (M19)
 *
 * `blame.ts` is the CodeMirror extension, `BlamePopup.tsx` is the card, `blameStore.ts` is the
 * fetch — and this is every decision the three of them share. The split is `chrome/sidebarWidth.ts`'s
 * and the rule for what belongs here is the one its header states: **values in, values out**.
 * Nothing here reads a clock, a DOM node or a store.
 *
 * # Deliberately import-free
 *
 * `scripts/check-blame.mjs` compiles this one file with the TypeScript in `node_modules` and
 * imports the output. A single `import type … from '@/ipc/generated'` would drag in the `@/*`
 * path alias, and behind it the whole IPC surface — so [`BlameFileLike`], [`BlameRunLike`] and
 * [`BlameCommitLike`] are **structural restatements** of the generated DTOs rather than those
 * types, exactly as `sidebarWidth.ts`'s `StoredSidebar` is of `SidebarSettings`.
 *
 * The restatement is not a second source of truth: `EditorPane.tsx` calls [`collapseRuns`] with
 * the real `BlameFile`, so a field renamed in `crates/cide-ipc/src/history.rs` is a type error at
 * that call site — and `check-blame.mjs` additionally slices the field names out of
 * `ui/src/ipc/generated.ts` and pins them, which is the join that catches a rename before it
 * shows up as `undefined` in a gutter.
 *
 * # Everything is in seconds, and `now` is always a parameter
 *
 * The wire carries unix **seconds** (`BlameCommit.authored`), so every number in this file is
 * seconds — never milliseconds, and never a `Date`. And `now` is passed in at every call rather
 * than read from `Date.now()`, because a clock inside a pure function makes every assertion about
 * it depend on the minute the check happened to run. It is the same reason `autosave.ts` takes its
 * facts as arguments.
 */

/**
 * The wire's `BlameCommit`, restated. See the header for why this is not an import.
 *
 * `authored` accepts `bigint | number` and is narrowed **once**, in [`seconds`]. ts-rs renders
 * Rust's `i64` as `bigint`, so this is what actually arrives; `number` is admitted so a fixture
 * and a hand-written call site do not each have to spell the `n`. The narrowing has to happen in
 * exactly one place — a per-caller `Number(…)` is how one of them forgets and renders
 * `Invalid Date`, and `bigint` is the type that makes that failure silent rather than loud
 * (`new Date(bigint)` throws, but `` `${bigint}` `` does not).
 */
export interface BlameCommitLike {
  readonly oid: string
  readonly shortOid: string
  readonly summary: string
  readonly author: string
  readonly authorEmail: string
  readonly authored: bigint | number
  /** The path the file had **at this commit**. Rust sends `null` when it equals `BlameFile.path`. */
  readonly origPath: string | null
  readonly boundary: boolean
}

/** The wire's `BlameRun`, restated. `commit` indexes [`BlameFileLike.commits`]. */
export interface BlameRunLike {
  /** First line of the run, **1-based**, like every other line number on this wire. */
  readonly start: number
  /** How many lines. Never zero. */
  readonly lines: number
  /** `null` for a line that is not committed — a local edit under `working`/`buffer`. */
  readonly commit: number | null
}

/** The wire's `BlameFile`, restated — the fields the column actually reads. */
export interface BlameFileLike {
  readonly path: string
  readonly lines: number
  readonly runs: readonly BlameRunLike[]
  readonly commits: readonly BlameCommitLike[]
  /**
   * Set when the follow mode Rust ran is weaker than the one asked for, saying why.
   *
   * Carried into every card rather than shown once somewhere else: the caveat is about the
   * *attribution*, so the place it has to appear is beside an attribution. A `-C -C` that was
   * abandoned and quietly answered as a plain rename-follow gives a gutter that looks like the
   * expensive one and is not, and the user has no way to tell.
   */
  readonly downgraded?: string | null
}

/** One line of the column. One of these exists for **every** line of the file. */
export interface BlameMarker {
  /** 1-based, matching `BlameRun.start` and CodeMirror's `Line.number`. */
  readonly line: number
  /** Full oid, or `''` for an uncommitted line — which is therefore not a click target. */
  readonly oid: string
  /**
   * The text in the cell. **Empty for every line of a run but the first.**
   *
   * IDEA's collapsing, and it is what makes a 22-character column legible rather than a wall of
   * the same name repeated forty times. The *tint* is not collapsed — see [`BlameMarker.bucket`]
   * — so a run still reads as one block; only the words stop repeating.
   */
  readonly label: string
  /**
   * The age tint. [`UNCOMMITTED_BUCKET`] for an uncommitted line, otherwise `0..AGE_BUCKETS.length`
   * inclusive — newest first.
   *
   * Carried by **every** line of a run, unlike the label. Two facts, two rules, and they are
   * deliberately different: the tint is what says "these forty lines are one change", and it can
   * say it on every row because it costs no space.
   */
  readonly bucket: number
  /**
   * The whole card, as plain text, one line per entry joined with `\n` — exactly
   * `popupLines(commit).join('\n')`.
   *
   * One string rather than an array because a `GutterMarker` is compared with `eq()` on every
   * gutter update, and comparing one string is cheaper than walking two arrays for each of a
   * viewport's worth of lines. `blame.ts` splits it again for the popup, so the hover card and
   * this value cannot disagree about what a line says.
   *
   * It is deliberately **not** written to the cell's `title` attribute: WebKitGTK would then draw
   * its own tooltip a second after ours, and the user would be looking at two cards.
   */
  readonly title: string
}

/**
 * Age boundaries, in seconds: a day, a week, a month, a quarter, a year.
 *
 * **Exponential, not linear, and that is the whole design.** Commit ages are: most of what is in
 * front of you was written in the last month and the rest was written at some point in the last
 * five years. A linear ramp over a two-year-old file puts every line in one bucket and says
 * nothing at all; these five boundaries put the recent churn — which is what a reader of a blame
 * gutter is looking for — across four of the six steps.
 *
 * Five boundaries means **six** buckets, `0..=5`. `AGE_BUCKETS.length` is the oldest.
 */
export const AGE_BUCKETS: readonly number[] = [
  86_400, // a day
  7 * 86_400, // a week
  30 * 86_400, // a month
  90 * 86_400, // a quarter
  365 * 86_400, // a year
]

/**
 * How wide the column is, in characters of the gutter's monospace face.
 *
 * 22 is a full ISO date (10), a space, and eleven characters of the author — `Ada Lovelac…
 * 2026-07-10`. The date is what a reader scans and the name is what they recognise, and eleven
 * characters is enough to recognise one; anything wider is margin taken from the buffer in every
 * split pane of every annotated file, which is the trade IDEA makes the same way.
 *
 * `EditorSurface.module.css` sizes the column as `22ch` and `check-blame.mjs` pins the two
 * together — a stylesheet that disagreed would either clip the date off every label or leave dead
 * space beside every line.
 */
export const BLAME_LABEL_CHARS = 22

/** [`BlameMarker.bucket`] for a line that is not committed. Never a valid age bucket. */
export const UNCOMMITTED_BUCKET = -1

/** The cell text on the first line of an uncommitted run. */
export const UNCOMMITTED_LABEL = 'Uncommitted'

/**
 * How long the pointer has to rest on a cell before the card appears.
 *
 * 250 ms rather than CodeMirror's own 300 ms hover default: this is a gutter, so the pointer is
 * travelling *across* rows rather than resting on a word, and the card is the only way to read a
 * summary that the 22-character cell cannot show. Long enough that scanning the column with the
 * mouse does not flash cards, short enough to feel like an answer rather than a wait.
 */
export const BLAME_HOVER_MS = 250

/**
 * The one place a wire timestamp becomes a JavaScript number. See [`BlameCommitLike.authored`].
 *
 * `Number(bigint)` is lossless for every second this side of the year 285616 (2^53 seconds), so
 * there is nothing to guard here beyond doing it exactly once.
 */
function seconds(authored: bigint | number): number {
  return typeof authored === 'bigint' ? Number(authored) : authored
}

/**
 * Which age band a commit falls in: `0` is newer than a day, `AGE_BUCKETS.length` is a year or
 * more. Monotonically **non-increasing** in `authored` — a more recent commit is never a higher
 * bucket.
 *
 * A future timestamp is bucket 0 rather than an error. Clock skew and a rebase both produce them
 * routinely, and the honest reading of "authored after now" is "as new as it gets"; the
 * alternative — a negative age falling through to the oldest bucket — would paint the newest line
 * in the file as the oldest, which is the failure that is hardest to notice because it looks
 * deliberate.
 */
export function ageBucket(authored: number, now: number): number {
  const age = now - authored
  if (!Number.isFinite(age) || age <= 0) return 0
  for (let i = 0; i < AGE_BUCKETS.length; i += 1) {
    const edge = AGE_BUCKETS[i]
    if (edge !== undefined && age < edge) return i
  }
  return AGE_BUCKETS.length
}

/** Two digits, zero-padded. */
function pad(n: number): string {
  return n < 10 ? `0${n}` : `${n}`
}

/**
 * `2026-08-19`, in the user's own timezone.
 *
 * Local rather than UTC because the question a blame gutter answers is "when did *I* write this",
 * and a commit made at 23:30 that shows yesterday's date is wrong in the only way the reader can
 * detect. `check-blame.mjs` pins `TZ=UTC` for itself so its assertions do not depend on the
 * machine.
 *
 * **Absolute, never relative**, and that is the house rule rather than a preference:
 * `sidebar/GitPanel/ShelfList.tsx` states it — *"A relative string has to be recomputed to stay
 * true, and a list that silently goes stale is worse than a date that is always right."* A gutter
 * is the worst possible place for `2h ago`, because it is on screen for as long as the file is.
 */
function isoDay(at: Date): string {
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`
}

/** `14:32`, local. */
function hourMinute(at: Date): string {
  return `${pad(at.getHours())}:${pad(at.getMinutes())}`
}

/** `2026-08-19 14:32`, local — the card's exact date, where there is room for both halves. */
function isoMinute(at: Date): string {
  return `${isoDay(at)} ${hourMinute(at)}`
}

/**
 * `Ivan Vorontsov 2026-08-19`, clipped to [`BLAME_LABEL_CHARS`].
 *
 * # Why `now` is a parameter and not a constant
 *
 * A commit made in the last day shows `14:32` instead of its date — git's own `--date=human`
 * rule. It is not decoration: the lines you are most likely to be annotating are the ones you or
 * the agent wrote in the last hour, and "today at 14:32" is the answer there, while the date is
 * the answer everywhere else. The five characters it saves go to the author's name.
 *
 * # The clip
 *
 * The **stamp** is what survives, and the name is what gives way. A name is recognisable from its
 * first eleven characters and a truncated date is not a date at all — `2026-08-1` is a different
 * and plausible-looking day. So the room left over after the stamp is the name's, and an
 * over-long one ends in `…`.
 */
export function blameLabel(author: string, authored: number, now: number): string {
  const at = new Date(authored * 1000)
  // `Invalid Date` renders as the literal string `Invalid Date` through every getter, which would
  // put `NaN-NaN-NaN` in the column. A cell that says nothing is better than a cell that lies.
  const stamp = Number.isFinite(at.getTime())
    ? now - authored < 86_400 && now - authored >= 0
      ? hourMinute(at)
      : isoDay(at)
    : ''
  const room = BLAME_LABEL_CHARS - stamp.length - (stamp === '' ? 0 : 1)
  const name = author.length > room ? `${author.slice(0, Math.max(0, room - 1))}…` : author
  if (stamp === '') return name
  return name === '' ? stamp : `${name} ${stamp}`
}

/**
 * The hover card's content, in order. **Line 0 is the heading** — `BlamePopup.tsx` renders it in
 * the mono face and the rest as detail rows, and that index rule is the whole contract between
 * the two files.
 *
 * The card is a list of lines rather than seven props for the same reason `branchModel.ts::explain`
 * is a function returning a sentence: the *content* of the card is a decision — which rows exist,
 * in which order, and which of them are conditional — and a decision spelled inside JSX is a
 * decision no check script can run. What is left in the component is the part that genuinely is
 * a view.
 *
 * `note` is `BlameFile.downgraded`, appended last so the caveat sits under the answer it qualifies.
 */
export function popupLines(
  commit: BlameCommitLike | null,
  note?: string | null,
): readonly string[] {
  const lines: string[] = []
  if (commit === null) {
    lines.push(UNCOMMITTED_LABEL)
    lines.push('Not in HEAD — this line exists only in your working copy.')
  } else {
    lines.push(`${commit.shortOid} ${commit.summary}`)
    lines.push(
      commit.authorEmail === '' ? commit.author : `${commit.author} <${commit.authorEmail}>`,
    )
    const at = new Date(seconds(commit.authored) * 1000)
    if (Number.isFinite(at.getTime())) lines.push(isoMinute(at))
    /*
     * Only when it differs — and "differs" is Rust's answer, not ours. `BlameCommit.origPath` is
     * documented as `None` when it equals `BlameFile.path`, so the comparison has already been
     * made on the side that knows how the repository spells both. Re-deriving it here from the
     * file's own path would be a second answer to a question that already has one, and it would
     * be the wrong one for a submodule, where the two paths are relative to different roots.
     */
    if (commit.origPath !== null) lines.push(`↳ was ${commit.origPath}`)
    /*
     * A boundary commit is the *walk's limit*, not necessarily where the line was written — a
     * shallow clone or an explicit `newest` produces one. Saying so is the difference between a
     * fact and a guess presented as one: without this row the card names a specific person for a
     * line they may never have seen.
     */
    if (commit.boundary) {
      lines.push('Boundary commit — the walk stopped here, so this may not be where it was written.')
    }
  }
  if (note !== undefined && note !== null && note !== '') lines.push(note)
  return lines
}

/**
 * The wire's runs into one marker per line, or `[]` if the run set is not a valid cover.
 *
 * # The invariant is enforced here, not assumed
 *
 * `cide_git::blame::check_runs` asserts ascending, gapless, covering `[1, lines]` on every route
 * that produces a `BlameFile`, and this checks it again on arrival. That is not distrust of the
 * backend — it is that **nothing downstream can notice a violation**. A run set with a hole paints
 * a gutter that is silently one line off for everything below it, and every line still carries *a*
 * label, so the column looks exactly as authoritative as a correct one. There is no rendering
 * artefact, no gap, no error: just a per-line falsehood, in the one surface whose entire purpose
 * is to be believed.
 *
 * So the answer to a malformed set is to draw **no column at all**. Refusing rather than repairing,
 * for the same reason: a repair (extend the previous run over the hole, clamp the overshoot) would
 * produce a plausible column that is wrong in a way nobody can see, which is worse than a missing
 * feature.
 *
 * An empty file returns `[]` too, and the two cases are indistinguishable from the outside. That
 * is fine: a file with no lines has no column either way.
 *
 * # `now`
 *
 * Sampled **once** by the caller and passed in, so every line of one paint is bucketed against one
 * instant. Reading the clock per line would let a slow paint straddle a bucket edge and tint two
 * lines of the same run differently.
 */
export function collapseRuns(file: BlameFileLike, now: number): BlameMarker[] {
  const total = file.lines
  if (!Number.isInteger(total) || total < 0) return []

  // First pass: is this a cover? Nothing is allocated until the whole set has been accepted, so a
  // rejected set costs one walk and no garbage.
  let expect = 1
  for (const run of file.runs) {
    if (!Number.isInteger(run.start) || !Number.isInteger(run.lines)) return []
    if (run.start !== expect || run.lines < 1) return []
    if (run.commit !== null) {
      if (!Number.isInteger(run.commit)) return []
      if (run.commit < 0 || run.commit >= file.commits.length) return []
    }
    expect += run.lines
  }
  if (expect - 1 !== total) return []

  const note = file.downgraded ?? null
  const markers: BlameMarker[] = []
  for (const run of file.runs) {
    const commit = run.commit === null ? null : (file.commits[run.commit] ?? null)
    /*
     * Built once per run and shared by every line of it. A 500-line run would otherwise build 500
     * identical cards and 500 identical strings, on a path that runs on every toggle of the
     * column over a file that may be twenty thousand lines long.
     */
    const title = popupLines(commit, note).join('\n')
    const oid = commit === null ? '' : commit.oid
    const bucket =
      commit === null ? UNCOMMITTED_BUCKET : ageBucket(seconds(commit.authored), now)
    const label =
      commit === null ? UNCOMMITTED_LABEL : blameLabel(commit.author, seconds(commit.authored), now)
    for (let i = 0; i < run.lines; i += 1) {
      markers.push({
        line: run.start + i,
        oid,
        // The collapsing rule, and the only place it is expressed.
        label: i === 0 ? label : '',
        bucket,
        title,
      })
    }
  }
  return markers
}
