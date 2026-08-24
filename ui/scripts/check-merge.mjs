/**
 * Checks `src/panes/mergeModel.ts` — the rules behind the three-pane conflict resolver. (M20)
 *
 * # What is worth pinning, and why it is this and not a merge algorithm
 *
 * This model does not merge anything. `mergeModel.ts`'s header carries the argument in full:
 * `cide_git::pull` runs a real `git merge` or `git rebase`, so the working-tree file **is** git's
 * merge result, and the model parses what git wrote rather than re-deriving a merge that could
 * disagree with the index cide is about to commit.
 *
 * So what can go silently wrong is parsing and rendering, and two of those failures are severe:
 *
 *   - **A conflict marker reaching `git add`.** `canApply` is the guard, and a count of answered
 *     regions is not enough on its own: the centre pane is editable, so a user can answer every
 *     region by button and still leave a `<<<<<<<` they typed or pasted. Marker soup commits
 *     cleanly and breaks the build for everybody, which makes this the worst thing this surface
 *     could do.
 *   - **An unanswered region rendering as one side.** The centre pane is editable and savable,
 *     so silently choosing a side would discard the other the moment somebody pressed Apply.
 *     Unanswered regions must round-trip back to markers, byte for byte.
 *
 * Run: `pnpm --dir ui run check:merge`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-merge-'))

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}
const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

try {
  // --- the wire shapes the pane reads ---------------------------------------------------------

  const generated = read('../src/ipc/generated.ts')
  for (const field of ['base', 'ours', 'theirs', 'ourLabel', 'theirLabel', 'binary', 'tooLarge']) {
    ok(
      new RegExp(`\\b${field}\\b`).test(generated),
      `ConflictFile.${field} is still its name on the wire`,
    )
  }
  ok(
    /export type ConflictSide = "base" \| "ours" \| "theirs"/.test(generated),
    'ConflictSide still spells its three stages the way the panel passes them',
  )
  ok(
    /export type MergeState = \{/.test(generated),
    'MergeState is still a struct — what the panel draws its bar from',
  )

  // --- compile the model on its own -----------------------------------------------------------

  const tsconfig = join(out, 'tsconfig.json')
  writeFileSync(
    tsconfig,
    JSON.stringify({
      compilerOptions: {
        target: 'es2022',
        module: 'esnext',
        moduleResolution: 'bundler',
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        verbatimModuleSyntax: true,
        skipLibCheck: true,
        noEmitOnError: true,
        outDir: out,
        rootDir: join(UI, 'src'),
        baseUrl: UI,
        paths: { '@/*': ['src/*'] },
        types: [],
      },
      files: [join(UI, 'src', 'panes', 'mergeModel.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', tsconfig], {
    stdio: 'inherit',
    cwd: UI,
  })

  const m = await import(`file://${join(out, 'panes', 'mergeModel.js')}`)

  // --- the diff ---------------------------------------------------------------------------------

  const L = (t) => t.split('\n')

  /*
   * Patience diff anchors on lines unique to both sides, which is what makes it produce the
   * hunks a person expects: it matches the distinctive line rather than the braces and blank
   * lines a shortest-edit algorithm happily pairs across unrelated blocks.
   */
  eq(m.diffLines(L('a\nb\nc'), L('a\nb\nc')), [], 'identical files have no edits')
  eq(
    m.diffLines(L('a\nb\nc'), L('a\nX\nc')),
    [{ aFrom: 1, aTo: 2, bFrom: 1, bTo: 2 }],
    'one changed line, with the common prefix and suffix trimmed off it',
  )
  eq(
    m.diffLines(L('a\nc'), L('a\nb\nc')),
    [{ aFrom: 1, aTo: 1, bFrom: 1, bTo: 2 }],
    'an insertion is a ZERO-WIDTH edit on the base side — which is what lets two insertions at ' +
      'the same line be recognised as one conflicting block',
  )
  eq(
    m.diffLines(L('a\nb\nc'), L('a\nc')),
    [{ aFrom: 1, aTo: 2, bFrom: 1, bTo: 1 }],
    'and a deletion is the mirror',
  )
  eq(m.diffLines([], L('a')), [{ aFrom: 0, aTo: 0, bFrom: 0, bTo: 1 }], 'an empty side')
  {
    // Two independent edits either side of an anchor the algorithm has to find.
    const edits = m.diffLines(L('one\nANCHOR\ntwo'), L('1\nANCHOR\n2'))
    eq(edits.length, 2, 'a unique common line anchors, and the two halves diff separately')
  }

  // --- the three-way build -------------------------------------------------------------------

  const base = 'a\nb\nc\nd\ne'

  {
    // Only ours changed line `b`.
    const doc = m.build(base, 'a\nOURS\nc\nd\ne', base)
    eq(doc.regions.length, 1, 'one changed block')
    eq(doc.conflicts.length, 0, 'and it is not a conflict — only one side touched it')
    eq(doc.regions[0].changedOurs, true, 'ours changed it')
    eq(doc.regions[0].changedTheirs, false, 'theirs did not')
    eq(doc.regions[0].ours, ['OURS'], "and the region carries that side's text")
    eq(doc.regions[0].theirs, ['b'], 'while the untouched side carries the base')

    /*
     * THE RULE THE WHOLE MODEL RESTS ON: with nothing decided, the result is the **base**.
     *
     * The first version of this module parsed git's conflict markers, so the hunks git had
     * already merged were invisible and unrevertable — a merge tool showing two decisions out
     * of thirty and calling the rest settled. Here every change is a block you take.
     */
    eq(m.renderResult(doc, {}), base, 'NOTHING DECIDED LEAVES THE BASE, unapplied')
    const took = { r0: { taken: ['ours'], ignored: [] } }
    eq(m.renderResult(doc, took), 'a\nOURS\nc\nd\ne', 'accepting applies that side')
    const dropped = { r0: { taken: [], ignored: ['ours'] } }
    eq(m.renderResult(doc, dropped), base, 'and rejecting keeps the base — it does NOT delete')
  }

  {
    // Both sides changed the same line, differently.
    const doc = m.build(base, 'a\nOURS\nc\nd\ne', 'a\nTHEIRS\nc\nd\ne')
    eq(doc.conflicts.length, 1, 'both sides, differently, is a conflict')
    eq(m.unresolvedCount(doc, {}), 1, 'and it blocks Apply')
    eq(m.canApply(base, doc, {}), false, 'so Apply is refused')

    const both = { r0: { taken: ['ours', 'theirs'], ignored: [] } }
    eq(
      m.renderResult(doc, both),
      'a\nOURS\nTHEIRS\nc\nd\ne',
      'BOTH SIDES, in the order they were clicked — the commonest real conflict there is',
    )
    eq(
      m.renderResult(doc, { r0: { taken: ['theirs', 'ours'], ignored: [] } }),
      'a\nTHEIRS\nOURS\nc\nd\ne',
      'and the other order gives the other file, which is why click order is recorded',
    )
    eq(m.canApply(m.renderResult(doc, both), doc, both), true, 'answered, so Apply is allowed')
  }

  {
    // Both sides made the SAME change. Not a conflict — git does not mark it as one either.
    const doc = m.build(base, 'a\nSAME\nc\nd\ne', 'a\nSAME\nc\nd\ne')
    eq(doc.conflicts.length, 0, 'the same edit from both sides is not a conflict')
    eq(doc.regions.length, 1, 'but it is still a block, so it can be seen and rejected')
  }

  {
    // Two insertions at the same base line — the case the user named: two people adding a
    // function or a test at the same place, where the answer is both.
    const doc = m.build('a\nz', 'a\nOURS\nz', 'a\nTHEIRS\nz')
    eq(doc.conflicts.length, 1, 'two insertions at one line OVERLAP and become one conflict')
    eq(
      m.renderResult(doc, { r0: { taken: ['ours', 'theirs'], ignored: [] } }),
      'a\nOURS\nTHEIRS\nz',
      'and both can be taken, one after the other',
    )
  }

  {
    // A side that deleted the file entirely.
    const doc = m.build(base, null, base)
    eq(doc.regions.length, 1, 'a deleted side is one region over the whole file')
    eq(doc.regions[0].ours, [], 'with no lines of its own')
    eq(
      m.renderResult(doc, { r0: { taken: ['ours'], ignored: [] } }),
      '',
      'and accepting it empties the file, which is what that side did',
    )
  }

  // --- decisions ---------------------------------------------------------------------------------

  {
    const doc = m.build(base, 'a\nOURS\nc\nd\ne', 'a\nTHEIRS\nc\nd\ne')
    const region = doc.regions[0]
    const none = { taken: [], ignored: [] }

    /*
     * THREE STATES PER SIDE, not two. A toggle was the first shape: clicking the chevron again
     * took the change back out, so the button never went away, and a user who clicked twice
     * watched the block vanish with no way to tell a decision from a slip.
     */
    eq(m.pending(region, none, 'ours'), true, 'undecided: the side still has buttons')
    const taken = m.accept(none, 'ours')
    eq(m.pending(region, taken, 'ours'), false, 'ACCEPTED: its buttons go away')
    const rejected = m.ignore(none, 'ours')
    eq(m.pending(region, rejected, 'ours'), false, 'REJECTED: its buttons go away too')
    eq(m.settled(region, taken), false, 'but the region is not settled while the other side asks')
    eq(
      m.settled(region, m.ignore(m.accept(none, 'ours'), 'theirs')),
      true,
      'and it is once both sides have been answered',
    )

    eq(m.accept(m.accept(none, 'ours'), 'theirs').taken, ['ours', 'theirs'], 'accept APPENDS')
    eq(
      m.accept(m.accept(none, 'ours'), 'ours').taken,
      ['ours'],
      'and accepting twice is idempotent rather than a toggle',
    )
    eq(m.ignore(m.accept(none, 'ours'), 'ours').taken, [], 'rejecting an accepted side takes it out')
    eq(m.ignore(m.accept(none, 'ours'), 'ours').ignored, ['ours'], 'and records the rejection')
    eq(m.accept(m.ignore(none, 'ours'), 'ours').ignored, [], 'and the reverse clears it')
    eq(m.reset(), none, 'reset puts a whole block back to asking')

    /*
     * `↺` on an answered side, which is what stops a decision being a dead end. It matters most
     * for blocks nobody decided: *Apply non-conflicting* settles a dozen at once, and a decision
     * made on somebody's behalf has to be reachable.
     */
    eq(m.answered(region, none, 'ours'), false, 'an unanswered side offers no revert')
    eq(m.answered(region, taken, 'ours'), true, 'an accepted one does')
    eq(m.answered(region, rejected, 'ours'), true, 'and so does a rejected one')
    eq(
      m.answered(m.build(base, base, 'a\nTHEIRS\nc\nd\ne').regions[0], taken, 'ours'),
      false,
      'but never a side that did not change the block — it has no say here',
    )
    eq(m.unset(taken, 'ours'), none, 'revert puts one side back to asking')
    eq(
      m.unset(m.accept(m.accept(none, 'ours'), 'theirs'), 'ours').taken,
      ['theirs'],
      'and leaves the other side alone, so "both" can be backed out of by half',
    )

    /*
     * A HIGHLIGHT IS WORK YOU HAVE NOT DONE. Two tones, and `null` — do not paint — the moment a
     * block is settled. Keeping the answered ones lit leaves a merge of thirty blocks with thirty
     * coloured bands and nothing to separate the two still to read from the twenty-eight already
     * dealt with.
     */
    eq(m.regionTone(region, none), 'conflict', 'undecided conflict: red')
    const oneSided = m.build(base, 'a\nOURS\nc\nd\ne', base)
    eq(
      m.regionTone(oneSided.regions[0], none),
      'pending',
      'an unanswered ONE-SIDED change is blue, not red — a change, not a problem',
    )
    eq(
      m.regionTone(region, m.ignore(m.accept(none, 'ours'), 'theirs')),
      null,
      'both sides answered: dark',
    )
    eq(
      m.regionTone(oneSided.regions[0], m.accept(none, 'ours')),
      null,
      'accepted: dark',
    )
    eq(
      m.regionTone(oneSided.regions[0], m.ignore(none, 'ours')),
      null,
      'discarded: dark — the colour is not a record of what was decided',
    )
    eq(
      m.regionTone(region, taken),
      'conflict',
      'but a conflict with only ONE side answered stays lit — the other half is still work',
    )
  }

  // --- the bulk actions --------------------------------------------------------------------------

  {
    const doc = m.build('a\nb\nc\nd', 'a\nOURS\nc\nd', 'a\nb\nc\nTHEIRS')
    eq(doc.regions.length, 2, 'two independent one-sided changes')
    eq(doc.conflicts.length, 0, 'neither is a conflict')
    const applied = m.applyNonConflicting(doc, {})
    eq(
      m.renderResult(doc, applied),
      'a\nOURS\nc\nTHEIRS',
      'Apply non-conflicting takes the one side of every block only one side touched',
    )

    const conflicted = m.build(base, 'a\nOURS\nc\nd\ne', 'a\nTHEIRS\nc\nd\ne')
    eq(
      m.applyNonConflicting(conflicted, {}),
      {},
      'and leaves conflicts entirely alone',
    )
    const held = { r0: { taken: [], ignored: ['ours'] } }
    eq(
      m.applyNonConflicting(doc, held).r0,
      held.r0,
      'and never undoes a rejection the user already made',
    )

    const twins = m.build(base, 'a\nSAME\nc\nd\ne', 'a\nSAME\nc\nd\ne')
    eq(m.resolveSimple(twins, {}), {}, 'identical edits are not conflicts, so there is none to resolve')
  }

  // --- markers and Apply -----------------------------------------------------------------------

  {
    const doc = m.build(base, 'a\nOURS\nc\nd\ne', base)
    /*
     * Apply waits for EVERY block, not only the conflicting ones.
     *
     * It used to light up as soon as the conflicts were answered, on the argument that leaving a
     * one-sided change keeps the base and is a valid outcome. It is a valid outcome and a
     * terrible default: the user is looking at a chevron and an `X` still sitting in the gutter,
     * has not decided about them, and the tool is telling them they are finished.
     */
    eq(m.openCount(doc, {}), 1, 'a one-sided change is an open block')
    eq(m.unresolvedCount(doc, {}), 0, 'even though it is not a conflict')
    eq(
      m.canApply(base, doc, {}),
      false,
      'AND IT BLOCKS APPLY — a block with buttons on it is a question nobody has answered',
    )
    eq(
      m.canApply(base, doc, { r0: { taken: [], ignored: ['ours'] } }),
      true,
      'rejecting it is the answer, and it is one click',
    )
    eq(
      m.canApply('a\nOURS\nc\nd\ne', doc, { r0: { taken: ['ours'], ignored: [] } }),
      true,
      'as is accepting it',
    )
    eq(
      m.canApply('ok\n<<<<<<< typed by hand', doc, { r0: { taken: [], ignored: ['ours'] } }),
      false,
      'but a marker in the text does, wherever it came from — marker soup commits cleanly and ' +
        'breaks the build for everybody',
    )
  }
  for (const marker of ['<<<<<<<', '>>>>>>>']) {
    eq(m.hasMarkers(`ok\n${marker} x\nok`), true, `hasMarkers catches ${marker}`)
  }
  /*
   * The two it deliberately ignores. A line of seven `=` is a setext heading underline in
   * Markdown and a section rule in reStructuredText — ordinary content in exactly the files
   * people conflict over — and refusing to save one would be a resolver that cannot write a
   * README.
   */
  eq(m.hasMarkers('Heading\n=======\n\ntext'), false, 'a Markdown setext heading is not a marker')
  eq(m.hasMarkers('a === b\nnot a marker'), false, 'and neither is ordinary code')

  // --- where the highlights and chevrons go ------------------------------------------------------

  {
    const doc = m.build(base, 'a\nOURS\nOURS2\nc\nd\ne', base)
    /*
     * `resultSpans` numbers lines into the text `renderResult` produces, and the two walk with
     * the same cursor for that reason. One line of drift paints a block's colour over the code
     * beside it, which reads as the tool being confused about what you are answering.
     */
    eq(m.resultSpans(doc, {}), [{ id: 'r0', from: 1, to: 2 }], 'undecided: the base line it covers')
    eq(
      m.resultSpans(doc, { r0: { taken: ['ours'], ignored: [] } }),
      [{ id: 'r0', from: 1, to: 3 }],
      'accepted: the two lines that side put there',
    )
    // Side spans come straight off the region now — the first version searched the side document
    // for the fragment's text, because the marker model had no positions to work from.
    eq(m.sideSpans(doc, 'ours'), [{ id: 'r0', from: 1, to: 3 }], "ours' own lines")
    eq(m.sideSpans(doc, 'theirs'), [{ id: 'r0', from: 1, to: 2 }], "and theirs, which is the base")
  }

  // --- source pins ---------------------------------------------------------------------------------------

  const model = read('../src/panes/mergeModel.ts')
  ok(
    !/^\s*import\s/m.test(model.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '')),
    'mergeModel.ts has NO imports — it is compiled alone by this script',
  )
  ok(
    !/parseConflicts|<<<<<<</.test(model.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '').replace(/hasMarkers[\s\S]*$/, '')),
    'and it no longer parses git\'s markers — the merge is computed from the three index stages, ' +
      'so the hunks git already applied are visible and revertible instead of invisible',
  )

  const pane = read('../src/panes/MergePane.tsx')
  const gutterSrc = read('../src/panes/mergeGutter.ts')
  ok(
    /build\(file\?\.base \?\? null, file\?\.ours \?\? null, file\?\.theirs \?\? null\)/.test(pane),
    'the pane builds the merge from base/ours/theirs — the index stages, not the working tree',
  )
  /*
   * `readOnly` alone, and the absence of `editable.of(false)` is the assertion.
   *
   * `DiffPane` uses both and is right to; this pane is not that. `editable.of(false)` takes the
   * caret away, and with it keyboard selection, Ctrl+A and Ctrl+C — and these panes are where
   * the text a user wants to copy *into* the result lives. A pane you cannot copy out of is a
   * pane you have to retype from. `readOnly` on its own refuses every document change while
   * leaving the selection and the clipboard exactly as they are in any editor.
   */
  ok(
    /\[EditorState\.readOnly\.of\(true\)\]/.test(pane),
    'the side panes are read-only through `readOnly`',
  )
  ok(
    !/EditorView\.editable\.of\(false\)/.test(pane),
    'and NOT through `editable.of(false)`, which would make them impossible to copy out of',
  )
  ok(
    /canApply\(result, doc, decisions\)/.test(pane),
    'Apply is gated on canApply rather than on a count',
  )
  ok(/lineNumbers\(\)/.test(pane), 'all three panes carry line numbers')
  ok(
    /conflictGutter\(side\)/.test(pane),
    'and the two side panes carry a chevron gutter',
  )
  /*
   * The chevrons are the tool's central gesture: a button per *pane* accepts a whole side, and
   * what a merge tool is for is taking **this** block from the left and **that** one from the
   * right.
   */
  ok(
    /setGutter\(/.test(pane) && /line: span\.from/.test(pane),
    'one chevron per block, on that block\'s own first line in that side — not one per pane',
  )
  ok(
    /const open = pending\(region, held, side\)/.test(pane)
      && /const done = answered\(region, held, side\)/.test(pane),
    'A CHEVRON GOES AWAY ONCE ITS SIDE IS ANSWERED, accepted or rejected — one that stayed ' +
      'looked like it had not worked, and clicking it again did something else entirely',
  )
  ok(
    /if \(!open && !done\) return \[\]/.test(pane),
    'and a side that did not change the block gets nothing at all — a button there would invite ' +
      "a decision about somebody else's edit",
  )
  ok(
    /line: span\.from, answered: done/.test(pane) && /cm-mergeRevert/.test(gutterSrc),
    'while an ANSWERED side gets `↺` rather than nothing, so a decision is never a dead end — ' +
      'which matters most for the ones *Apply non-conflicting* made on the user\'s behalf',
  )
  ok(
    /what === 'accept'\s*\? accept\(held, side\)\s*: what === 'ignore'\s*\? ignore\(held, side\)\s*: unset\(held, side\)/
      .test(pane.replace(/\s+/g, ' ')),
    'and a button answers the block it is ON, with the side it is IN — accept, reject or revert',
  )

  ok(
    /other instanceof ChevronMarker && other\.id === this\.id && other\.glyph === this\.glyph/.test(
      gutterSrc.replace(/\s+/g, ' '),
    ),
    'ChevronMarker.eq ignores the handler — comparing closures would rebuild every button on ' +
      'every render and drop the pointer mid-click',
  )
  ok(
    /side: which === 'ours' \? 'after' : 'before'/.test(gutterSrc),
    "the left pane's chevrons sit on its right edge and the right pane's mirror them, so both " +
      'point at the result',
  )
  ok(
    /cm-mergeIgnore/.test(gutterSrc) && /'ignore'/.test(gutterSrc),
    "and each block gets IDEA's `X` beside the chevron — rejecting is a decision, not the " +
      'absence of one',
  )
  // The four limitations this pane shipped with, each now pinned as fixed.
  ok(/inlineDiff\(/.test(pane), 'word-level detail inside a changed line')
  /*
   * Only the result pane is painted. The side panes were tinted too and the colour was noise
   * there — and the empty `paint(view, [])` is deliberate rather than a missing call:
   * `paintedField` holds whatever it was last given, so a pane that stops being painted has to
   * be told, or a band from before a decision stays on screen for ever.
   */
  ok(
    /const lit = spans\.filter/.test(pane)
      && /pending\(region, decisionOf\(decisions, span\.id\), side\)/.test(pane),
    'a side pane lights the blocks IT has not answered — per side, so answering the left half of ' +
      'a conflict quietens the left pane while the right stays lit',
  )
  ok(
    /if \(tone === null\) return \[\]/.test(pane),
    'and every pane skips a settled block rather than painting it a second colour',
  )
  /*
   * The insertion markers. (M25) A block a pane has no lines for — the other side inserted
   * where it has nothing, or it deleted the block — draws the boundary as a thin line
   * rather than lying a full tone band onto the neighbouring line, and rather than (the
   * worse, older state) drawing nothing at all in the pane that lacked the lines.
   */
  const decoSrc = read('../src/panes/mergeDecorations.ts')
  ok(
    /span\.from === span\.to/.test(decoSrc) && /cm-mergeInsert /.test(decoSrc),
    'a zero-width span paints as a cm-mergeInsert marker line, not as a band on the line beside it',
  )
  ok(
    /cm-mergeInsertEnd/.test(decoSrc),
    'with the end-of-document variant, because an insertion below the last line has no ' +
      'following line to carry a top edge',
  )
  ok(
    /const inserts = spans\.filter/.test(pane) && /span\.from !== span\.to/.test(pane),
    'the side panes append their zero-width spans — `lit` cannot carry a block this side did ' +
      'not change, which is exactly the theirs-only insertion that used to be invisible here',
  )
  ok(
    /\[\.\.\.lit, \.\.\.inserts\]/.test(pane),
    'through the same paint call and the same regionTone gate, so a settled block draws nothing',
  )
  {
    const mergeCss = read('../src/panes/MergePane.module.css')
    ok(
      /\.cm-mergeInsert\)\s*\{[^}]*position:\s*relative/.test(mergeCss) &&
        /\.cm-mergeInsert\)::before\s*\{[^}]*height:\s*2px/.test(mergeCss),
      'the marker is a 2px ::before with no layout height — CodeMirror’s height cache and the ' +
        'block-anchored scroll sync must see nothing',
    )
    ok(
      /\.cm-mergeInsertConflict\)::before\s*\{[^}]*var\(--red\)/.test(mergeCss) &&
        /\.cm-mergeInsertPending\)::before\s*\{[^}]*var\(--blue\)/.test(mergeCss),
      'in the pane’s existing two tones — red for an undecided conflict, blue for an ' +
        'unanswered one-sided change — not a third colour language',
    )
    ok(
      /\.cm-mergeInsertEnd\)::before\s*\{[^}]*bottom:\s*-1px/.test(mergeCss),
      'and the end variant moves the line to the bottom edge',
    )
  }
  ok(
    /scrollDOM\.addEventListener\('scroll'/.test(pane) && /echoes\.current\.add\(to\)/.test(pane),
    'the panes scroll together, with a re-entry guard so the sync cannot feed itself',
  )
  ok(
    /history\.current\.past/.test(pane) && /key !== 'z'/.test(pane),
    'Ctrl+Z walks back through the block decisions, which the editor\'s own history does not cover',
  )
  ok(
    !/if \(span\.to === span\.from\) return \[\]/.test(pane),
    'and a side that DELETED a block still gets a chevron — it was skipped for having no lines ' +
      'of its own, which left "take the side that deleted this" reachable only from a heading',
  )
  /*
   * Syntax highlighting, through the same style the editor paints with — a merge showing the
   * file in different colours from the tab beside it would read as a different file. A
   * compartment, not a baked-in extension: the grammar is a dynamic import that is not there
   * when the editor is built, and rebuilding to add it would take the scroll and selection.
   */
  ok(
    /syntaxHighlighting\(cideHighlightStyle\)/.test(pane),
    'all three panes use the editor\'s own highlight style',
  )
  ok(
    /languageSlots\.current\[side\]\?\.of\(\[\]\)/.test(pane)
      && /slot\.reconfigure\(extension\)/.test(pane),
    'and the grammar arrives through a compartment once its chunk lands',
  )
  ok(
    /views\.current\[side\] !== view/.test(pane),
    'guarded on the view still being live — a tab closed mid-import would otherwise dispatch ' +
      'into a destroyed editor',
  )
  ok(
    /scrollDOM\.getBoundingClientRect\(\)\.top - view\.documentTop/.test(pane),
    'the scroll sync converts through `documentTop` — `scrollTop` is not a document-relative ' +
      'height, and reading it as one was off by the padding on every pane',
  )
  ok(
    /const echoes = useRef<Set<string>>/.test(pane) && /echoes\.current\.delete\(from\)/.test(pane),
    'and the re-entry guard is a SET OF MARKS rather than a timer — a programmatic `scrollTop` ' +
      'fires its event a frame or more later, so a flag released on the next frame was already ' +
      'down when the echo arrived, and the three panes converged on line one',
  )
  ok(
    /overflow: hidden/.test(read('../src/panes/MergePane.module.css').split('.paneBody {')[1] ?? ''),
    'and the pane body does not scroll, so CodeMirror\'s own scroller is the only one',
  )
  ok(
    /afterResolve\(project, repo/.test(pane),
    'both exits from the resolver go back to the conflict list — or commit, if that was the last ' +
      'file',
  )

  const inline = m.inlineDiff(['const a = getUser(id)'], ['const a = getUserById(id)'], 0)
  eq(inline.b.length, 1, 'one changed run on the line')
  eq(
    'const a = getUserById(id)'.slice(inline.b[0].from, inline.b[0].to),
    'getUserById',
    'and it is the identifier that changed, not the whole line',
  )
  eq(m.inlineDiff(['same'], ['same'], 0).b, [], 'an unchanged line has no runs')
  eq(
    m.inlineDiff(['a'], ['a', 'b'], 0).b,
    [],
    'and lines past the shorter side have no counterpart to compare against, so the block tint ' +
      'carries them',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} failure(s)`)
  process.exit(1)
}
console.log('merge model: ok')
