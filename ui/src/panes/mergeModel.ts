/**
 * The three-way merge behind the resolver: what changed, who changed it, and what wins. (M20)
 *
 * # The centre starts as the base, and every change is a decision
 *
 * IDEA's model, and its documentation states it plainly: *"Initially, the contents of this pane
 * are the same as the base revision of the file, that is, the revision from which both
 * conflicting versions are derived."* Every difference either side made is a block with a
 * chevron; you take the ones you want.
 *
 * This module's first shape did something else — it parsed git's conflict markers, on the
 * argument that git had already merged the non-conflicting hunks and re-deriving them risked
 * disagreeing with the index. That argument is sound and it produced the wrong tool: the hunks
 * git had already applied were **invisible and unrevertable**, so a merge tool could show you
 * two decisions out of thirty and call the rest settled. A merge tool exists to show all of
 * them.
 *
 * So the merge is computed here, from the three index stages, and the working tree's markers are
 * not read at all. What the old argument was really protecting — that the resolver and the
 * commit cannot disagree — is preserved by construction from the other end: **the resolved text
 * is written to the working tree and staged verbatim**, so what the user sees is what gets
 * committed, whatever any algorithm thought.
 *
 * # No imports
 *
 * `ui/scripts/check-merge.mjs` compiles this file alone and drives it under node, so it carries
 * no imports at all — not even type-only ones. `chrome/logActions.ts` sets that precedent.
 */

/** One version of a changed region. */
export type Side = 'ours' | 'theirs'

/**
 * Which sides have been taken into the result, in click order — or `null` for *not applied*.
 *
 * Three states, and the difference between two of them is the point:
 *
 * * `null` — untouched. The **base** stands for this region, which is what makes the centre pane
 *   open as the base revision and every change an opt-in.
 * * `[]` — *take neither*, explicitly. The region is deleted, which is a different file from the
 *   one `null` gives and a real answer when both sides added something unwanted.
 * * `['ours', 'theirs']` — both, **in the order clicked**, because that is the order they appear
 *   in the file. The commonest real conflict is two people adding a function, a test or an
 *   import at the same line, where the answer is neither side but both. JetBrains describes the
 *   same case: *"the change on the other side will remain open… to combine the changes from both
 *   sides, you can choose to accept them both."*
 */
export type Choice = readonly Side[] | null

/** One block that at least one side changed. */
export interface Region {
  /** Stable within one build: `r0`, `r1`, … in document order. */
  readonly id: string
  /** The lines of the base this region covers. Zero-based, end-exclusive. */
  readonly baseFrom: number
  readonly baseTo: number
  /** What this region looks like in `ours`, and where. */
  readonly ours: readonly string[]
  readonly oursFrom: number
  readonly oursTo: number
  readonly theirs: readonly string[]
  readonly theirsFrom: number
  readonly theirsTo: number
  /** Whether that side changed the base here at all. */
  readonly changedOurs: boolean
  readonly changedTheirs: boolean
  /** Both sides changed it, and not to the same thing. */
  readonly conflict: boolean
}

export interface MergeDoc {
  readonly base: readonly string[]
  readonly regions: readonly Region[]
  /** Just the conflicting ones — what must be answered before the file can be written. */
  readonly conflicts: readonly Region[]
}

// --- building ---------------------------------------------------------------------------------

/**
 * The three-way merge of `base`, `ours` and `theirs`.
 *
 * A missing side — `null`, meaning that side deleted the file — is treated as empty, which makes
 * the whole file one region and both answers reachable. A missing base is the same: two sides
 * that added the same path independently have no common ancestor, and every line of both is a
 * change.
 */
export function build(
  base: string | null,
  ours: string | null,
  theirs: string | null,
): MergeDoc {
  const b = lines(base)
  const o = lines(ours)
  const t = lines(theirs)

  const groups = group(diffLines(b, o), diffLines(b, t))

  const regions: Region[] = []
  let basePos = 0
  let oursPos = 0
  let theirsPos = 0

  for (const g of groups) {
    // The run neither side touched, which advances all three cursors together. This is what
    // keeps `oursFrom`/`theirsFrom` exact without a second pass mapping positions.
    const gap = g.from - basePos
    basePos += gap
    oursPos += gap
    theirsPos += gap

    const oursText = window(b, o, g.ours, g.from, g.to)
    const theirsText = window(b, t, g.theirs, g.from, g.to)
    const changedOurs = g.ours.length > 0
    const changedTheirs = g.theirs.length > 0

    regions.push({
      id: `r${regions.length}`,
      baseFrom: g.from,
      baseTo: g.to,
      ours: oursText,
      oursFrom: oursPos,
      oursTo: oursPos + oursText.length,
      theirs: theirsText,
      theirsFrom: theirsPos,
      theirsTo: theirsPos + theirsText.length,
      changedOurs,
      changedTheirs,
      // Both sides having edited is not enough: two people making the *same* edit is not a
      // conflict, and git does not mark it as one either.
      conflict: changedOurs && changedTheirs && !same(oursText, theirsText),
    })

    basePos = g.to
    oursPos += oursText.length
    theirsPos += theirsText.length
  }

  return { base: b, regions, conflicts: regions.filter((r) => r.conflict) }
}

function lines(text: string | null): string[] {
  return text === null || text === '' ? [] : text.split('\n')
}

function same(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((line, i) => line === b[i])
}

/** One side's version of a base window, with that side's edits applied into it. */
function window(
  base: readonly string[],
  side: readonly string[],
  edits: readonly Edit[],
  from: number,
  to: number,
): string[] {
  const out: string[] = []
  let at = from
  for (const edit of edits) {
    if (edit.aFrom > at) out.push(...base.slice(at, edit.aFrom))
    out.push(...side.slice(edit.bFrom, edit.bTo))
    at = edit.aTo
  }
  if (at < to) out.push(...base.slice(at, to))
  return out
}

// --- grouping ---------------------------------------------------------------------------------

/**
 * One changed block, as ranges into the two line arrays. Zero-based, end-exclusive.
 *
 * Exported since M35, for `editor/changeModel.ts` — the gutter's change markers classify a
 * `diffLines` result into added/modified/deleted and need the shape to say so. Exporting a type
 * adds no import, which is what keeps this file compilable on its own; see the header.
 */
export interface Edit {
  aFrom: number
  aTo: number
  bFrom: number
  bTo: number
}

interface Group {
  from: number
  to: number
  ours: Edit[]
  theirs: Edit[]
}

/**
 * Fold the two edit lists into regions, merging anything that overlaps or touches.
 *
 * Touching counts, not only overlapping, and that is what makes the case the user described
 * work: two insertions at the same base line are both zero-width at that position, so they
 * overlap trivially and become **one** conflicting region offering both — rather than two
 * regions that would each replace the other.
 */
function group(ours: readonly Edit[], theirs: readonly Edit[]): Group[] {
  const all = [
    ...ours.map((e) => ({ edit: e, side: 'ours' as const })),
    ...theirs.map((e) => ({ edit: e, side: 'theirs' as const })),
  ].sort((x, y) => x.edit.aFrom - y.edit.aFrom || x.edit.aTo - y.edit.aTo)

  const groups: Group[] = []
  for (const { edit, side } of all) {
    const last = groups[groups.length - 1]
    if (last !== undefined && edit.aFrom <= last.to) {
      last.to = Math.max(last.to, edit.aTo)
      last[side].push(edit)
      continue
    }
    const fresh: Group = { from: edit.aFrom, to: edit.aTo, ours: [], theirs: [] }
    fresh[side].push(edit)
    groups.push(fresh)
  }
  // Each side's edits must be in base order for `window` to walk them once.
  for (const g of groups) {
    g.ours.sort((x, y) => x.aFrom - y.aFrom)
    g.theirs.sort((x, y) => x.aFrom - y.aFrom)
  }
  return groups
}

// --- the line diff ------------------------------------------------------------------------------

/**
 * Patience diff over lines: the changed blocks of `a` against `b`.
 *
 * Patience rather than the textbook LCS, and the reason is size as much as quality. A dynamic
 * table is `O(n·m)` cells, which for two 12,000-line files — the resolver's own limit — is 144
 * million, in a webview, on a keystroke. Patience anchors on lines that appear **exactly once on
 * each side**, takes the longest increasing run of those, and recurses between them; the common
 * prefix and suffix trim first, which on a real conflicted file collapses almost all of it.
 *
 * It is also what `git diff --patience` does, and it produces the hunks a person expects: it
 * anchors on the distinctive lines — a signature, an import — rather than on the braces and blank
 * lines that a shortest-edit algorithm happily matches across unrelated blocks.
 */
export function diffLines(a: readonly string[], b: readonly string[]): Edit[] {
  const out: Edit[] = []
  walk(a, b, 0, a.length, 0, b.length, out, 0)
  return out
}

/** How deep the anchor recursion may go before it gives up and emits one block. */
const MAX_DEPTH = 40

function walk(
  a: readonly string[],
  b: readonly string[],
  aFrom: number,
  aTo: number,
  bFrom: number,
  bTo: number,
  out: Edit[],
  depth: number,
): void {
  let lo = aFrom
  let bLo = bFrom
  while (lo < aTo && bLo < bTo && a[lo] === b[bLo]) {
    lo += 1
    bLo += 1
  }
  let hi = aTo
  let bHi = bTo
  while (hi > lo && bHi > bLo && a[hi - 1] === b[bHi - 1]) {
    hi -= 1
    bHi -= 1
  }
  if (lo === hi && bLo === bHi) return
  // One side is empty: a pure insertion or deletion, and there is nothing left to anchor on.
  if (lo === hi || bLo === bHi || depth >= MAX_DEPTH) {
    out.push({ aFrom: lo, aTo: hi, bFrom: bLo, bTo: bHi })
    return
  }

  const anchors = uniqueAnchors(a, b, lo, hi, bLo, bHi)
  if (anchors.length === 0) {
    out.push({ aFrom: lo, aTo: hi, bFrom: bLo, bTo: bHi })
    return
  }

  let at = lo
  let bAt = bLo
  for (const anchor of anchors) {
    walk(a, b, at, anchor.a, bAt, anchor.b, out, depth + 1)
    at = anchor.a + 1
    bAt = anchor.b + 1
  }
  walk(a, b, at, hi, bAt, bHi, out, depth + 1)
}

/**
 * Lines that occur exactly once in each range, paired, longest increasing run only.
 *
 * "Exactly once on both sides" is what makes an anchor trustworthy: a line that appears twice
 * gives no information about which copy corresponds to which. The increasing run is what keeps
 * the pairs in order — two anchors that cross would describe a move, which a diff cannot express
 * and which patience therefore discards.
 */
function uniqueAnchors(
  a: readonly string[],
  b: readonly string[],
  aFrom: number,
  aTo: number,
  bFrom: number,
  bTo: number,
): { a: number; b: number }[] {
  const inA = new Map<string, number>()
  for (let i = aFrom; i < aTo; i += 1) {
    const line = a[i] as string
    inA.set(line, inA.has(line) ? -1 : i)
  }
  const inB = new Map<string, number>()
  for (let i = bFrom; i < bTo; i += 1) {
    const line = b[i] as string
    inB.set(line, inB.has(line) ? -1 : i)
  }

  const pairs: { a: number; b: number }[] = []
  for (const [line, ai] of inA) {
    if (ai < 0) continue
    const bi = inB.get(line)
    if (bi === undefined || bi < 0) continue
    pairs.push({ a: ai, b: bi })
  }
  pairs.sort((x, y) => x.a - y.a)

  // Longest increasing subsequence on `b`, patience-sorting style. `tails[k]` is the index into
  // `pairs` of the smallest `b` that can end an increasing run of length `k + 1`.
  const tails: number[] = []
  const back: number[] = new Array(pairs.length).fill(-1)
  for (let i = 0; i < pairs.length; i += 1) {
    const value = (pairs[i] as { b: number }).b
    let lo = 0
    let hi = tails.length
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if ((pairs[tails[mid] as number] as { b: number }).b < value) lo = mid + 1
      else hi = mid
    }
    back[i] = lo > 0 ? (tails[lo - 1] as number) : -1
    tails[lo] = i
  }
  const run: { a: number; b: number }[] = []
  let at = tails.length > 0 ? (tails[tails.length - 1] as number) : -1
  while (at >= 0) {
    run.push(pairs[at] as { a: number; b: number })
    at = back[at] as number
  }
  return run.reverse()
}

// --- decisions ----------------------------------------------------------------------------------

/**
 * What has been done about one region, per side.
 *
 * # Why a side has three states and not two
 *
 * A toggle was the first shape and it was wrong twice over. Clicking `»` a second time took the
 * change back out — so the button never went away, and a user who clicked it twice watched the
 * block vanish from the result with no way to tell whether that was a decision or a slip. IDEA
 * has an **accept** and an **ignore** per side, and once either is used that side's buttons for
 * that block are gone: the block is settled for that side, and settled is a state a toggle
 * cannot represent.
 *
 * So: `taken` is what goes into the result, in click order; `ignored` is what was rejected. A
 * side in neither is still asking. Both lists rather than one map, because the *order* of
 * `taken` is load-bearing — see [`Choice`] — and a map has none.
 */
export interface Decision {
  readonly taken: readonly Side[]
  readonly ignored: readonly Side[]
}

export type Decisions = Readonly<Record<string, Decision>>

const UNDECIDED: Decision = { taken: [], ignored: [] }

export function decisionOf(decisions: Decisions, id: string): Decision {
  return decisions[id] ?? UNDECIDED
}

/** Take this side into the result. Appends, so accepting both keeps the order they were clicked. */
export function accept(decision: Decision, side: Side): Decision {
  return {
    taken: decision.taken.includes(side) ? decision.taken : [...decision.taken, side],
    ignored: decision.ignored.filter((s) => s !== side),
  }
}

/** Reject this side. The base stands for it, and its buttons go away. */
export function ignore(decision: Decision, side: Side): Decision {
  return {
    taken: decision.taken.filter((s) => s !== side),
    ignored: decision.ignored.includes(side) ? decision.ignored : [...decision.ignored, side],
  }
}

/** Put a whole region back to asking. The toolbar's *Reset*. */
export function reset(): Decision {
  return UNDECIDED
}

/**
 * Put **one side** back to asking — the gutter's `↺`.
 *
 * Without this an answered side is a dead end: its `»` and `✕` are gone, and the only way back is
 * the toolbar's Reset, which acts on whichever block the toolbar happens to be on. IDEA keeps a
 * revert control on an applied change for exactly this reason, and it matters most for the blocks
 * the user never decided at all — *Apply non-conflicting*, or the setting that runs it on open,
 * settles a dozen at once, and a decision made on your behalf has to be reachable.
 */
export function unset(decision: Decision, side: Side): Decision {
  return {
    taken: decision.taken.filter((s) => s !== side),
    ignored: decision.ignored.filter((s) => s !== side),
  }
}

/** Whether this side has been answered, either way — so the gutter offers `↺` instead. */
export function answered(region: Region, decision: Decision, side: Side): boolean {
  const changed = side === 'ours' ? region.changedOurs : region.changedTheirs
  return changed && !pending(region, decision, side)
}

/** Whether a side still has buttons: it changed something here, and nobody has said what to do. */
export function pending(region: Region, decision: Decision, side: Side): boolean {
  const changed = side === 'ours' ? region.changedOurs : region.changedTheirs
  return changed && !decision.taken.includes(side) && !decision.ignored.includes(side)
}

/** Whether every side that changed this region has been answered. */
export function settled(region: Region, decision: Decision): boolean {
  return !pending(region, decision, 'ours') && !pending(region, decision, 'theirs')
}

// --- rendering -----------------------------------------------------------------------------------

/**
 * One region's lines, given what has been decided about it.
 *
 * **Nothing taken leaves the base**, which is the rule that makes the centre pane open as the
 * base revision and every change an opt-in. It is also what fixes the disappearing block: taking
 * a side and then rejecting it puts the base back, rather than deleting the region — deletion is
 * what you get by accepting a side that deleted it, which is a decision somebody made rather
 * than the by-product of two clicks.
 */
export function regionText(doc: MergeDoc, region: Region, decision: Decision): string[] {
  if (decision.taken.length === 0) return doc.base.slice(region.baseFrom, region.baseTo)
  const out: string[] = []
  for (const side of decision.taken) {
    out.push(...(side === 'ours' ? region.ours : region.theirs))
  }
  return out
}

/** The centre pane's text. */
export function renderResult(doc: MergeDoc, decisions: Decisions): string {
  const out: string[] = []
  let at = 0
  for (const region of doc.regions) {
    out.push(...doc.base.slice(at, region.baseFrom))
    out.push(...regionText(doc, region, decisionOf(decisions, region.id)))
    at = region.baseTo
  }
  out.push(...doc.base.slice(at))
  return out.join('\n')
}

/** One region's line span in the text [`renderResult`] just produced. */
export interface Span {
  readonly id: string
  readonly from: number
  readonly to: number
}

/**
 * Where each region sits in the rendered result.
 *
 * Walked with the same cursor `renderResult` uses, because these are line numbers *into that
 * text*: a second rule here would drift from it silently, and one line of drift paints a
 * region's colour over the code beside it.
 */
export function resultSpans(doc: MergeDoc, decisions: Decisions): Span[] {
  const spans: Span[] = []
  let line = 0
  let at = 0
  for (const region of doc.regions) {
    line += region.baseFrom - at
    const text = regionText(doc, region, decisionOf(decisions, region.id))
    spans.push({ id: region.id, from: line, to: line + text.length })
    line += text.length
    at = region.baseTo
  }
  return spans
}

/**
 * Where each region sits in one whole side of the file.
 *
 * Read straight off the region now. The first version searched the side document for the
 * fragment's text, because the marker-parsing model had no positions to work from; computing the
 * merge here means the positions fall out of it exactly, and a search that could miss is gone.
 */
export function sideSpans(doc: MergeDoc, side: Side): Span[] {
  return doc.regions.map((region) => ({
    id: region.id,
    from: side === 'ours' ? region.oursFrom : region.theirsFrom,
    to: side === 'ours' ? region.oursTo : region.theirsTo,
  }))
}

/**
 * What a region should be painted as, or `null` for *do not paint it*.
 *
 * # One rule, in all three panes: a highlight is work you have not done
 *
 * A block is lit while it is still asking and goes dark the moment that side is accepted or
 * discarded. That is the whole meaning of the colour — *look here* — and it is why there are two
 * tones and not four. A merge of thirty blocks that keeps painting the answered ones ends with
 * thirty coloured bands and nothing to separate the two you have not read from the twenty-eight
 * you have; the colour stops being a signal at exactly the point it is needed.
 *
 * It is not a record of what you decided. The result column *is* that record — it holds the text
 * you chose — and the gutter's `↺` marks every side you answered, in the pane you answered it in.
 *
 * * `conflict` — both sides changed it differently and nobody has decided. Red.
 * * `pending` — one side changed it and has not been answered. Blue: a change, not a problem.
 *
 * The side panes ask this **per side** (`pending`), so answering the left half of a conflict
 * quietens the left pane while the right stays lit; the result asks it per region (`settled`),
 * because a block is only finished there once both sides are.
 */
export function regionTone(region: Region, decision: Decision): 'conflict' | 'pending' | null {
  if (settled(region, decision)) return null
  return region.conflict ? 'conflict' : 'pending'
}

// --- the bulk actions ------------------------------------------------------------------------------

/**
 * IDEA's *Apply All Non-Conflicting Changes*.
 *
 * Every region only one side touched is accepted; conflicts are left alone. This is also what
 * `Settings › Git › Apply non-conflicting changes automatically` runs when the pane opens.
 *
 * Regions already decided are left as they are — re-running it must not undo a rejection.
 */
export function applyNonConflicting(doc: MergeDoc, decisions: Decisions, only?: Side): Decisions {
  const next: Record<string, Decision> = { ...decisions }
  for (const region of doc.regions) {
    if (region.conflict) continue
    const decision = decisionOf(next, region.id)
    if (decision.taken.length > 0 || decision.ignored.length > 0) continue
    const side: Side | null = region.changedOurs ? 'ours' : region.changedTheirs ? 'theirs' : null
    if (side === null) continue
    if (only !== undefined && side !== only) continue
    next[region.id] = accept(decision, side)
  }
  return next
}

/**
 * IDEA's *Resolve simple conflicts*: the ones whose two sides say the same thing.
 *
 * Narrow on purpose. A conflict where both sides made the identical change has exactly one
 * answer; anything else needs a person, and a button that guessed would be worse than no button.
 */
export function resolveSimple(doc: MergeDoc, decisions: Decisions): Decisions {
  const next: Record<string, Decision> = { ...decisions }
  for (const region of doc.conflicts) {
    const decision = decisionOf(next, region.id)
    if (decision.taken.length > 0 || decision.ignored.length > 0) continue
    if (same(region.ours, region.theirs)) next[region.id] = accept(decision, 'ours')
  }
  return next
}

// --- finishing -------------------------------------------------------------------------------------

/** Conflicts nobody has answered. */
export function unresolvedCount(doc: MergeDoc, decisions: Decisions): number {
  return doc.conflicts.filter((r) => !settled(r, decisionOf(decisions, r.id))).length
}

/**
 * Blocks of **any** kind that still have a button waiting — conflicting or not.
 *
 * This is what gates *Apply*, and the distinction from [`unresolvedCount`] is the whole of a
 * reported bug. Apply used to light up as soon as the conflicts were answered, on the argument
 * that leaving a one-sided change unanswered simply keeps the base and is a valid outcome. It is
 * a valid outcome and it is a terrible **default**: the user is looking at a chevron and an `X`
 * still sitting in the gutter, has not decided about them, and the tool is telling them they are
 * finished. A block with buttons on it is a question that has not been asked yet.
 *
 * So Apply waits until every `»`, `«` and `✕` is gone. Rejecting is one click and *is* the answer
 * for a change you want to keep the base for.
 */
export function openCount(doc: MergeDoc, decisions: Decisions): number {
  return doc.regions.filter((r) => !settled(r, decisionOf(decisions, r.id))).length
}

/**
 * What the toolbar says.
 *
 * Counts **every** block, because every one of them gates Apply. The conflicts are named
 * separately when there are any, since they are the ones that need thought rather than a glance.
 */
export function progressLabel(doc: MergeDoc, decisions: Decisions): string {
  const total = doc.regions.length
  if (total === 0) return 'No changes'
  const open = openCount(doc, decisions)
  const done = `${total - open} of ${total} blocks resolved`
  const conflicts = unresolvedCount(doc, decisions)
  return conflicts === 0 ? done : `${done} · ${conflicts} conflicting`
}

/**
 * Whether the result can be written back.
 *
 * **Every block answered** — see [`openCount`] for why that is every block and not only the
 * conflicting ones — and no conflict marker anywhere in the text. The second is not redundant:
 * the centre pane is a real editor, so a marker can arrive by typing or pasting, and staging
 * marker soup is the worst thing this surface could do — it commits cleanly and breaks the build
 * for everybody.
 */
export function canApply(text: string, doc: MergeDoc, decisions: Decisions): boolean {
  if (openCount(doc, decisions) > 0) return false
  return !hasMarkers(text)
}

/**
 * Whether any line opens or closes a conflict.
 *
 * **Only `<<<<<<<` and `>>>>>>>`, deliberately not `=======` or `|||||||`.** A line of seven or
 * more `=` is a setext heading underline in Markdown and a section rule in reStructuredText —
 * ordinary content in files people conflict over constantly — and refusing to write a resolved
 * file because it contains a Markdown heading would be a resolver that cannot save a README.
 */
export function hasMarkers(text: string): boolean {
  return text.split('\n').some((line) => line.startsWith('<<<<<<<') || line.startsWith('>>>>>>>'))
}

/** The next region after `from` that still has a decision to make, wrapping. */
export function nextOpen(doc: MergeDoc, decisions: Decisions, from: string | null): string | null {
  const ids = doc.regions.map((r) => r.id)
  if (ids.length === 0) return null
  const start = from === null ? -1 : ids.indexOf(from)
  for (let step = 1; step <= ids.length; step += 1) {
    const region = doc.regions[(start + step + ids.length) % ids.length]
    if (region !== undefined && !settled(region, decisionOf(decisions, region.id))) return region.id
  }
  return null
}

// --- word-level detail inside a changed line ------------------------------------------------------

/** A run of characters that differs, within one line. */
export interface Inline {
  /** Zero-based line, in whichever document this was computed for. */
  readonly line: number
  readonly from: number
  readonly to: number
}

/**
 * The parts of a region that actually differ, character by character.
 *
 * IDEA dims the unchanged text of a modified line and highlights only what moved; a whole-block
 * tint says *something here is different* and leaves the reader to find it, which on a
 * reformatted line or a renamed identifier is most of the work.
 *
 * Computed per line-pair, not across the block: two blocks of unequal length have no line
 * correspondence to speak of, and pairing line 3 of one with line 3 of the other would highlight
 * noise. Lines past the shorter side's end are wholly different and are left to the block tint.
 *
 * Word boundaries, not characters. A character diff over `getUser` → `getUserById` marks the
 * three-letter tail, which is right; over `alpha` → `beta` it marks a scatter of shared vowels,
 * which is unreadable. Splitting on identifier boundaries first gives the answer a person would.
 */
export function inlineDiff(
  a: readonly string[],
  b: readonly string[],
  lineOffset: number,
): { a: Inline[]; b: Inline[] } {
  const outA: Inline[] = []
  const outB: Inline[] = []
  const pairs = Math.min(a.length, b.length)
  for (let i = 0; i < pairs; i += 1) {
    const left = a[i] ?? ''
    const right = b[i] ?? ''
    if (left === right) continue
    const lw = words(left)
    const rw = words(right)
    // Common prefix and suffix in whole words. What is left in the middle is the change.
    let head = 0
    while (head < lw.length && head < rw.length && lw[head]?.text === rw[head]?.text) head += 1
    let tail = 0
    while (
      tail < lw.length - head &&
      tail < rw.length - head &&
      lw[lw.length - 1 - tail]?.text === rw[rw.length - 1 - tail]?.text
    ) {
      tail += 1
    }
    const span = (list: { text: string; at: number }[], line: string): Inline | null => {
      const first = list[head]
      const last = list[list.length - 1 - tail]
      if (first === undefined || last === undefined || head > list.length - 1 - tail) {
        // One side is wholly contained in the other — a pure insertion or deletion at the join.
        // Marking the whole line would be a lie about the half that did not move, so the
        // insertion point is marked instead, as a zero-width run the renderer widens.
        const at = list[head]?.at ?? line.length
        return { line: lineOffset + i, from: at, to: at }
      }
      return { line: lineOffset + i, from: first.at, to: last.at + last.text.length }
    }
    const sa = span(lw, left)
    const sb = span(rw, right)
    if (sa !== null) outA.push(sa)
    if (sb !== null) outB.push(sb)
  }
  return { a: outA, b: outB }
}

/** A line as words and the runs between them, each with its offset. */
function words(line: string): { text: string; at: number }[] {
  const out: { text: string; at: number }[] = []
  let at = 0
  // Identifier-ish runs, whitespace runs, and everything else one character at a time. Splitting
  // punctuation individually is what lets `a.b(c)` → `a.b(d)` mark only the `c`.
  const pattern = /[A-Za-z0-9_$]+|\s+|[^A-Za-z0-9_$\s]/g
  let match = pattern.exec(line)
  while (match !== null) {
    out.push({ text: match[0], at: match.index })
    at = match.index + match[0].length
    match = pattern.exec(line)
  }
  if (out.length === 0) out.push({ text: '', at })
  return out
}
