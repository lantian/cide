/**
 * What a buffer changed against HEAD, as blocks a gutter can paint. (M35)
 *
 * IDEA's change markers: a bar beside every line this buffer added or rewrote, a caret where it
 * removed lines, and — through [`revertPlan`] — the arithmetic that puts any one of them back.
 *
 * # Every decision is here, and the reason is that every failure is silent
 *
 * A marker one line off still looks authoritative. A revert that eats the wrong line break leaves
 * a blank line behind, or swallows a line the user wrote. A baseline split on the wrong character
 * paints an entire CRLF file blue and reads exactly like a feature working on a file that has
 * genuinely changed. None of it throws, none of it is visible in a screenshot, and there is no
 * test runner in this project — so the parts that can be decided over plain values are decided
 * here, where `scripts/check-change-bars.mjs` compiles the file on its own and drives them.
 *
 * # Deliberately import-free
 *
 * Not even the diff. `panes/mergeModel.ts` owns the patience diff and is import-free for its own
 * check's sake; importing it here would be harmless at runtime and fatal to the standalone
 * compile, so [`changeBlocks`] takes the edit list as an argument and [`EditLike`] restates its
 * shape. The join that catches a rename is the call site in `editor/changeBars.ts` — typed
 * against the real `Edit` — plus the check, which compiles *both* modules and composes them
 * exactly as the app does.
 *
 * # The BOM, which is the trap that is not here
 *
 * `cide_core::document::read` decodes strictly and `cide_git::revision::file_at_revision` decodes
 * lossily, and **neither strips a leading U+FEFF**; CodeMirror does not strip one either. So a
 * BOM'd file carries the same invisible character at the head of line 1 on both sides and
 * compares equal. Stripping it from one side — which is the obvious "defensive" edit — is what
 * would make line 1 differ for ever on every such file.
 *
 * A lossy decode is symmetric for the same reason: the working copy reaches the buffer through a
 * lossy path too, so identical invalid bytes become identical U+FFFD on both sides. The residual
 * case is two *different* invalid sequences collapsing to the same replacement character and
 * comparing equal, which is a marker that is missing rather than a marker that is wrong.
 */

/** What kind of change a block is, and which colour the gutter paints for it. */
export type ChangeKind = 'added' | 'modified' | 'deleted'

/**
 * `panes/mergeModel.ts`'s `Edit`, restated. See the header for why this is not an import.
 *
 * `a` is the baseline, `b` is the buffer. Zero-based, end-exclusive.
 */
export interface EditLike {
  readonly aFrom: number
  readonly aTo: number
  readonly bFrom: number
  readonly bTo: number
}

/** One run of the buffer that differs from HEAD. */
export interface ChangeBlock {
  readonly kind: ChangeKind
  /**
   * First buffer line the block covers. **1-based, inclusive.**
   *
   * For a `deleted` block there is nothing to cover: the removed text sat *above* this line, and
   * `lastLine` is `firstLine - 1` to say so.
   */
  readonly firstLine: number
  /** Last buffer line the block covers, inclusive. Less than `firstLine` for a deletion. */
  readonly lastLine: number
  /**
   * The HEAD lines this block replaced. Empty for `added`.
   *
   * This is what the popup shows and what Revert puts back, so it is carried on the block rather
   * than re-derived at click time: the baseline array can be swapped under a live buffer by a
   * commit landing in another window, and a card that read the *new* baseline while describing a
   * block computed against the old one is a per-block falsehood with nothing to notice it.
   */
  readonly baseLines: readonly string[]
  /**
   * A deletion that sat at the very end of the file, so there is no line below it to hang the
   * caret on and no *trailing* break for the revert to eat.
   *
   * A separate flag rather than a comparison against the line count at paint time, because the
   * buffer moves between the recompute and the click and the answer must not move with it.
   */
  readonly atEnd: boolean
}

/**
 * How to put one block back, in line numbers.
 *
 * Line numbers and not offsets, so this stays decidable over plain arrays and the check can apply
 * a plan to a `string[]` and assert the result equals the baseline. `changeBars.ts` translates to
 * offsets against the live document at dispatch time.
 */
export interface RevertPlan {
  /** First line to replace, 1-based inclusive. */
  readonly fromLine: number
  /** Last line to replace, inclusive — **less than `fromLine` for a pure insertion.** */
  readonly toLine: number
  /** The lines to put in their place. Empty when the revert only removes. */
  readonly insert: readonly string[]
  /**
   * Which surrounding line break the edit has to consume, and this field is the whole reason
   * this function exists.
   *
   * A line's text excludes its break, so removing lines 4–6 by replacing `[start of 4, end of 6]`
   * with nothing leaves the break that ended line 3 *and* the one that ended line 6 — an empty
   * line where the block was. Every add/delete shape therefore has to eat exactly one break, and
   * **which one depends on whether the block runs to the end of the file**: at EOF there is no
   * trailing break, so the leading one goes instead.
   */
  readonly eat: 'none' | 'trailing' | 'leading'
}

/**
 * Above this many lines on either side, no markers at all.
 *
 * The diff is a patience diff whose common prefix and suffix are trimmed first, so a one-line
 * edit in a 40,000-line file costs a scan and almost nothing else — the cap is against the
 * pathological case (two files that share nothing) rather than against size as such. It is
 * deliberately far above `panes/DiffPane.tsx`'s `MAX_DIFF_LINES` of 12,000, because that number
 * bounds a *rendering* — every line of both sides as a DOM element — and this bounds an array
 * walk. The gutter itself is viewport-windowed and realises nothing off screen.
 *
 * The byte gate is separate and lives in `EditorSurface`: `HIGHLIGHT_LIMIT_BYTES`, the same flag
 * that already decides a buffer gets no grammar, no folding and no bracket matching. Two gates,
 * because the two pathological shapes — very many short lines, and very few enormous ones — each
 * slip past the other. `DiffPane` pairs its own two for that reason.
 */
export const CHANGE_MAX_LINES = 50_000

/** How many baseline lines the popup shows before it says how many more there are. */
export const POPUP_MAX_LINES = 40

/**
 * The gutter column's width, in `ch`, pinned against the stylesheet by the check.
 *
 * `ch` and not px for `--fold-col`'s stated reason: it resolves against the gutter's own font, so
 * the strip follows `editor.fontSize` instead of becoming a sliver in a 20px buffer.
 */
export const CHANGE_COL_CH = 1

/**
 * How long a burst of typing collects before the markers are recomputed.
 *
 * A **trailing throttle, not a debounce** — `editor/docSync.ts` and `chrome/gitCountStore.ts`
 * both write out the reason at length: a restarting debounce is starved by continuous input, so
 * somebody typing steadily would see the bars frozen at whatever they were when the burst began.
 * A throttle fires on a fixed cadence for as long as the burst lasts, which is what "live while
 * typing" has to mean.
 *
 * 120 ms is the number `blameStore` and `gitStatusStore` already throttle at, and it sits well
 * inside `blameModel`'s `BLAME_HOVER_MS` of 250 — this codebase's own reading of the human band.
 * A bar that appears within 120 ms of the keystroke that caused it reads as live.
 */
export const CHANGE_RECOMPUTE_MS = 120

/**
 * Split the HEAD blob into lines, on all three break shapes.
 *
 * **This is the highest-value function in the file.** `EditorState.create` splits on `/\r\n?|\n/`
 * and `Text.toString()` rejoins with `\n`, so the buffer is LF-only from the first frame whatever
 * is on disk — `editor/lineEndings.ts` exists to put the file's own shape back on save. The blob,
 * meanwhile, is the committed bytes. Split it on `'\n'` alone and every baseline line keeps a
 * trailing `\r`, every comparison fails, and **the whole file paints modified** — with no
 * exception, no artefact and nothing logged, looking precisely like a working feature on a file
 * that has genuinely changed.
 *
 * # The trailing empty line is **kept**, and this is where the rule differs from the diff pane's
 *
 * `panes/diffRows.ts`' `splitLines` drops it, as Rust's `strip_newline` does, and both are right:
 * they are lining up against *git's* line numbering, where a file ending in a break has no line
 * after it. This function is lining up against a **CodeMirror document**, and `Text` keeps that
 * line — `"a\nb\n"` is three lines, the last of them empty. Copying the diff pane's rule here
 * gives a baseline one line shorter than the buffer for every file that ends in a newline, which
 * is very nearly every file in a repository, and the diff reports a phantom **addition on the
 * last line** of each of them. It is a green bar at the bottom of files nobody has touched, in a
 * column whose whole purpose is to be believed, and it is invisible until somebody scrolls to the
 * end.
 *
 * So the split is exactly CodeMirror's: the three break shapes, and every part kept. `''` is one
 * empty line, which is what an empty document is.
 */
export function splitBaseline(text: string): string[] {
  return text.split(/\r\n|\r|\n/)
}

/**
 * The blocks, from a diff of the baseline against the buffer.
 *
 * `edits` is `mergeModel.diffLines(base, buffer)`. The classification is the whole of the rule —
 * an empty range on the baseline side is an insertion, an empty range on the buffer side is a
 * deletion, anything else is a rewrite — and the *line arithmetic* around it is where the silent
 * bugs live, so it is written once here rather than at each of the three call sites that would
 * otherwise need it.
 *
 * Lines outside the buffer are **dropped rather than clamped**. A clamp produces a marker in a
 * plausible place, which is worse than no marker: `collapseRuns` refuses the same way, for the
 * same reason.
 */
export function changeBlocks(
  base: readonly string[],
  buffer: readonly string[],
  edits: readonly EditLike[],
): ChangeBlock[] {
  if (base.length > CHANGE_MAX_LINES || buffer.length > CHANGE_MAX_LINES) return []
  const out: ChangeBlock[] = []
  for (const edit of edits) {
    const added = edit.aFrom === edit.aTo
    const deleted = edit.bFrom === edit.bTo
    // Both empty is not a change at all. `diffLines` does not emit one, but a caller composing
    // this with a different differ would, and an empty block paints a marker on a line nothing
    // happened to.
    if (added && deleted) continue
    const baseLines = added ? [] : base.slice(edit.aFrom, edit.aTo)
    const firstLine = edit.bFrom + 1
    if (deleted) {
      // The removed text sat above `firstLine`. At the end of the file there is no line below it,
      // so the caret anchors on the last line instead and `atEnd` says which edge to draw on.
      const atEnd = edit.bFrom >= buffer.length
      const anchor = atEnd ? buffer.length : firstLine
      if (anchor < 1 || anchor > buffer.length) continue
      out.push({
        kind: 'deleted',
        firstLine: anchor,
        lastLine: anchor - 1,
        baseLines,
        atEnd,
      })
      continue
    }
    const lastLine = edit.bTo
    if (firstLine < 1 || lastLine > buffer.length || lastLine < firstLine) continue
    out.push({
      kind: added ? 'added' : 'modified',
      firstLine,
      lastLine,
      baseLines,
      atEnd: lastLine >= buffer.length,
    })
  }
  return out
}

/**
 * How to put one block back — see [`RevertPlan`], whose `eat` field carries the argument.
 *
 * `docLines` is the buffer's current line count, which decides the EOF cases. It is passed rather
 * than read off the block, because the buffer moves between the recompute and the click and the
 * plan has to describe the document as it is *now*.
 */
export function revertPlan(block: ChangeBlock, docLines: number): RevertPlan {
  if (block.kind === 'modified') {
    // Both the replaced region and the inserted text exclude their surrounding breaks, so this
    // one needs no break juggling at all.
    return {
      fromLine: block.firstLine,
      toLine: block.lastLine,
      insert: block.baseLines,
      eat: 'none',
    }
  }
  if (block.kind === 'added') {
    // Removing lines: one break has to go with them, or an empty line is left behind. The
    // trailing one normally; the leading one when the block runs to the end of the file, where
    // there is no trailing break to take.
    const runsToEnd = block.lastLine >= docLines
    return {
      fromLine: block.firstLine,
      toLine: block.lastLine,
      insert: [],
      eat: runsToEnd ? 'leading' : 'trailing',
    }
  }
  /*
   * A deletion: a pure insertion, which `toLine < fromLine` says.
   *
   * The insertion point is the anchor line — the text sat *above* it — and the restored lines
   * carry a **trailing** break so the anchor keeps its own line.
   *
   * At the end of the file the anchor *is* the last line and the text belongs below it, so the
   * point is `docLines + 1`: one past the end, which `applyRevert`'s `splice` appends at and
   * which `changeBars.ts` translates to `doc.length`. The break then goes in **front**, because
   * the last line has no trailing break to reuse. Writing this as "the last line, eat leading"
   * instead — the shape it had first — inserts the restored text *above* the final line, which
   * is off by exactly one line and looks right in every case where the last line is blank.
   */
  const at = block.atEnd ? Math.max(docLines, 1) + 1 : block.firstLine
  return {
    fromLine: at,
    toLine: at - 1,
    insert: block.baseLines,
    eat: block.atEnd ? 'leading' : 'trailing',
  }
}

/**
 * Apply a plan to an array of lines.
 *
 * Exported for the check, which round-trips every block shape — apply each plan to the buffer and
 * assert the result equals the baseline exactly. That assertion is the only automated defence on
 * the one operation here that can *destroy* the user's text rather than mispaint it, so it is
 * worth an export that the app itself never calls.
 *
 * `changeBars.ts` does not use this: it dispatches a CodeMirror transaction against a live `Text`
 * instead. That the two agree is a source pin, not a proof — see the check's header.
 */
export function applyRevert(lines: readonly string[], plan: RevertPlan): string[] {
  const out = lines.slice()
  const from = plan.fromLine - 1
  const count = plan.toLine < plan.fromLine ? 0 : plan.toLine - plan.fromLine + 1
  out.splice(from, count, ...plan.insert)
  return out
}

/** What the card for one block says. */
export interface PopupContent {
  /** The heading: `3 lines removed`, `2 lines changed`, `Line 12 added`. */
  readonly heading: string
  /** The HEAD lines to show, already truncated. */
  readonly lines: readonly string[]
  /** How many more there were. Zero when nothing was cut. */
  readonly hiddenLines: number
  /**
   * Whether a Copy button makes sense.
   *
   * **False for an added block**, because HEAD had nothing there: a Copy that copies the empty
   * string is a control that silently does nothing, which is the failure this project names most
   * often. The button is then omitted rather than disabled — `BlamePopup`'s rule.
   */
  readonly copyable: boolean
}

/**
 * What the card says, decided here rather than in JSX.
 *
 * `blameModel.popupLines` makes the argument: which rows a card has is a decision, and a decision
 * spelled inside a component is a decision no check script can run.
 */
export function popupContent(block: ChangeBlock): PopupContent {
  const removed = block.baseLines.length
  const covered = block.lastLine < block.firstLine ? 0 : block.lastLine - block.firstLine + 1
  const heading =
    block.kind === 'deleted'
      ? plural(removed, 'line removed', 'lines removed')
      : block.kind === 'added'
        ? plural(covered, 'line added', 'lines added')
        : plural(covered, 'line changed', 'lines changed')
  const shown = block.baseLines.slice(0, POPUP_MAX_LINES)
  return {
    heading,
    lines: shown,
    hiddenLines: removed - shown.length,
    copyable: removed > 0,
  }
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`
}
