/**
 * Checks `src/sidebar/clipboardModel.ts`, `src/sidebar/fsError.ts` and
 * `src/chrome/pasteConfirmModel.ts` — every decision the file tree's Copy / Cut / Paste makes
 * before it calls Rust, what the collision dialog asks, and how the result is reported back.
 *
 * All three in one script rather than a fourth `check:*` beside it: they are one gesture. The
 * dialog's answers become `pasteRefusal`'s command and `pastedSummary`'s sentence, and a rule
 * split across two scripts is one that gets changed in one of them.
 *
 * Same shape as `check-new-entry.mjs` beside it, and for the same reason: this project has no
 * JS test runner, and the rules worth pinning are pure functions whose wrong versions all
 * type-check. Each of these is invisible in a screenshot and expensive to get wrong:
 *
 *   * **where a paste lands** — a file row means its *parent*, exactly as *New File…* does,
 *     because two rules that agree today and are written twice disagree after the next change;
 *   * **"into itself"** as component-wise containment, since the string version of it refuses
 *     a perfectly good paste into a sibling whose name shares a prefix (`src` / `srcx`);
 *   * **what is said afterwards**, whose whole design is that it says *nothing* unless the
 *     result differed from what was asked for;
 *   * **`fsMessage`**, because `FsError` arrives as `{kind, detail}` with no `message` field
 *     and `String(error)` on one prints `[object Object]` — which is how a refusal the user
 *     could act on became a message that reads like a bug in the panel.
 *
 * Run: `pnpm --dir ui run check:fs-clipboard`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-fs-clipboard-'))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/clipboardModel.ts',
      'src/sidebar/fsError.ts',
      // Import-free and DOM-free by design, exactly like `chrome/closeConfirmModel.ts` — which is
      // what lets it be compiled here beside a module that does import something.
      'src/chrome/pasteConfirmModel.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // CommonJS and `node10`, like `check-picker.mjs`, and for the reason that script does not
      // have to explain: this module imports `./newEntry` (deliberately — the paste target and
      // the New File target are one function). The app compiles under `bundler` resolution,
      // where an extensionless specifier is normal; ESM output would keep it extensionless and
      // node would refuse to load it. `require` resolves it the old way.
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      // Both flags the app's own tsconfig sets. `noUncheckedIndexedAccess` is what makes
      // `clip.paths[0]` a `string | undefined`, which is the difference between a label
      // reading `“main.rs”` and one reading `“undefined”`.
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      // No DOM is needed here, but `@types/react-dom` is auto-included from `node_modules`
      // and does not compile without it. Same line, same reason, as `check-picker.mjs`.
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const m = require(join(out, 'sidebar', 'clipboardModel.js'))
  const errors = require(join(out, 'sidebar', 'fsError.js'))
  const ask = require(join(out, 'chrome', 'pasteConfirmModel.js'))

  let failed = 0
  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }
  const ok = (cond, what) => {
    if (!cond) {
      console.error(`FAIL ${what}`)
      failed++
    }
  }

  const ROOTS = ['/home/u/work/cide']
  const clip = (mode, paths, project = 'p1') => ({ mode, project, paths })

  // ---- where a paste lands ---------------------------------------------------------------

  eq(
    m.pasteTargetFor({ path: '/home/u/work/cide/src/main.rs', isDir: false }, ROOTS).parent,
    '/home/u/work/cide/src',
    'a FILE row pastes into its parent — the same rule New File follows, and the one edge that '
      + 'decides whether the feature feels right: "inside the thing I clicked" has no meaning '
      + 'for a file',
  )
  eq(
    m.pasteTargetFor({ path: '/home/u/work/cide/src', isDir: true }, ROOTS).parent,
    '/home/u/work/cide/src',
    'a FOLDER row pastes into that folder',
  )
  eq(
    m.pasteTargetFor(null, ROOTS).parent,
    '/home/u/work/cide',
    'no row — the empty space under the last row — pastes into the project root',
  )
  eq(m.pasteTargetFor(null, []), null, 'and a project with no roots has nowhere to paste')

  // ---- "into itself" ---------------------------------------------------------------------

  ok(m.isInside('/a/b', '/a/b'), 'a folder is inside itself')
  ok(m.isInside('/a/b/c', '/a/b'), 'and so is everything under it')
  ok(!m.isInside('/a/bc', '/a/b'), 'but a SIBLING whose name shares a prefix is not — the '
    + 'string version of this refuses pasting `src` into `srcx`, which is a legal paste')
  ok(!m.isInside('/a', '/a/b'), 'and a parent is not inside its child')

  // ---- the refusals ----------------------------------------------------------------------

  const target = m.pasteTargetFor({ path: '/home/u/work/cide/src', isDir: true }, ROOTS)

  eq(m.pasteRefusal(clip('copy', ['/home/u/work/cide/a.rs']), 'p1', target), null,
    'an ordinary paste is not refused')
  ok(m.pasteRefusal(null, 'p1', target).includes('Nothing has been copied'),
    'an empty clipboard is a REASON on a disabled item, not a hidden item — an item that '
      + 'appears only sometimes teaches the user nothing about why')
  ok(m.pasteRefusal(clip('copy', ['/x/a.rs'], 'p2'), 'p1', target).includes('another project'),
    'pasting into a different project is refused here, where it can be a sentence, rather '
      + 'than by ops::check_within after the gesture')
  ok(m.pasteRefusal(clip('copy', ['/home/u/work/cide/a.rs']), null, target).includes('No project'),
    'and with no project open there is nothing to paste into')
  ok(
    m.pasteRefusal(clip('cut', ['/home/u/work/cide/src']), 'p1', target).includes('into itself'),
    'a folder cannot be pasted into itself — the refusal that is not a refused gesture but an '
      + 'unbounded recursion',
  )
  ok(
    m
      .pasteRefusal(
        clip('cut', ['/home/u/work/cide/src']),
        'p1',
        m.pasteTargetFor({ path: '/home/u/work/cide/src/deep', isDir: true }, ROOTS),
      )
      .includes('into itself'),
    'nor into anything inside it',
  )
  ok(
    m.pasteRefusal(
      clip('copy', ['/home/u/work/cide/src']),
      'p1',
      m.pasteTargetFor({ path: '/home/u/work/cide/srcx', isDir: true }, ROOTS),
    ) === null,
    'and the sibling with the shared prefix is allowed',
  )

  // ---- labels ----------------------------------------------------------------------------

  eq(m.pasteLabel(clip('copy', ['/home/u/work/cide/a.rs']), target), 'Paste “a.rs” into src',
    'the destination is named in the label, because a file row pastes somewhere other than '
      + 'the row that was clicked')
  eq(m.pasteLabel(clip('copy', ['/a/1.rs', '/a/2.rs']), target), 'Paste 2 items into src',
    'several paths are counted rather than listed in a 252px panel')
  eq(m.pasteLabel(null, target), 'Paste', 'with nothing on the clipboard it is just Paste')
  eq(m.copyLabel('cut', 1), 'Cut', 'one item is not "Cut 1 Items"')
  eq(m.copyLabel('copy', 3), 'Copy 3 Items', 'and several are counted')

  // ---- the pending cut -------------------------------------------------------------------

  eq(m.pendingNote(clip('copy', ['/a/x.rs'])), null,
    'a COPY says nothing: it changes nothing until it is pasted, and a permanent strip under '
      + 'a 252px panel is a cost with no news in it')
  ok(m.pendingNote(clip('cut', ['/a/x.rs'])).includes('“x.rs”'),
    'a CUT is a pending removal and says so, naming what will move')
  ok(m.pendingNote(clip('cut', ['/a/x.rs'])).includes('Esc'),
    'and says how to call it off — a cut that cannot be cancelled is a trap')
  eq(m.pendingNote(null), null, 'an empty clipboard says nothing')

  ok(m.isCutPending(clip('cut', ['/a/x.rs']), '/a/x.rs'), 'a cut row is drawn faded')
  ok(!m.isCutPending(clip('copy', ['/a/x.rs']), '/a/x.rs'), 'a copied row is not')
  ok(!m.isCutPending(null, '/a/x.rs'), 'and neither is any row with an empty clipboard')

  // Escape cancels exactly what the two rules above draw, and nothing else.
  ok(m.escapeCancels(clip('cut', ['/a/x.rs']), 'p1'),
    'Escape calls off a cut in this project — the one thing the strip promises it can')
  ok(!m.escapeCancels(clip('copy', ['/a/x.rs']), 'p1'),
    'but NOT a copy: a copy is deliberately silent, so cancelling it changes nothing on screen '
      + 'and the next Ctrl+V says "nothing has been copied yet" — which is what a Copy that '
      + 'never worked looks like')
  ok(!m.escapeCancels(clip('cut', ['/a/x.rs'], 'p2'), 'p1'),
    'nor a cut made in another project, whose strip and faded rows are in the other panel')
  ok(!m.escapeCancels(null, 'p1'), 'and an empty clipboard leaves Escape to whoever wants it')

  eq(m.clipboardText(['/a/x.rs', '/a/y.rs']), '/a/x.rs\n/a/y.rs',
    'Copy also puts the paths on the SYSTEM clipboard, one per line, so a copy in the tree '
      + 'is usable in a terminal pane — cide does not put file references there, and cannot')

  // ---- what is said after a paste --------------------------------------------------------

  const pasted = (over) => ({ source: '/a/main.rs', dest: '/b/main.rs', renamed: false, skipped: 0, replaced: 0, ...over })

  eq(m.pastedSummary([pasted()], 'copy'), null,
    'the ORDINARY paste says nothing at all: the tree scrolls to and selects what it made, so '
      + 'a sentence describing it is noise')
  eq(m.pastedSummary([], 'copy'), null, 'and a paste of nothing says nothing')
  ok(
    m
      .pastedSummary([pasted({ dest: '/b/main copy.rs', renamed: true })], 'copy')
      .includes('“main copy.rs”'),
    'a RENAME must be said — the user asked for main.rs in a folder that has one, and the row '
      + 'they were looking at still holds the older file, so silence reads as "it did nothing"',
  )
  ok(
    m.pastedSummary([pasted({ renamed: true }), pasted({ renamed: true })], 'copy').includes('2 names'),
    'several renames are counted rather than listed',
  )
  ok(m.pastedSummary([pasted({ skipped: 2 })], 'copy').includes('2 sockets or pipes'),
    'skipped special files are reported: they are simply absent from the copy')
  ok(
    m.pastedSummary([pasted({ source: '/b/main.rs', dest: '/b/main.rs' })], 'cut')
      .includes('already in this folder'),
    'a cut pasted where it already was is a legitimate no-op and looks exactly like a broken '
      + 'Ctrl+V unless it says so',
  )
  ok(
    m.pastedSummary([pasted({ dest: '/b/main copy.rs', renamed: true, skipped: 1 })], 'cut')
      .includes('moved file'),
    'a renamed CUT does not call the result a copy — it is the file itself, moved',
  )

  ok(
    m.pastedSummary([pasted({ replaced: 1 })], 'copy').includes('1 existing file was replaced'),
    'a REPLACE is reported even though the user agreed to it: they agreed per folder, and a '
      + 'merge overwrites files one level down that the tree never showed them',
  )
  ok(
    m.pastedSummary([pasted({ replaced: 12 })], 'copy').includes('12 existing files'),
    'and the count is the receipt for the number the dialog quoted',
  )

  // ---- the collision dialog --------------------------------------------------------------
  //
  // > *"Paste collisions - yes, should be a confirmation"*
  //
  // What shipped renamed silently: `main.rs` became `main copy.rs`, which never loses data and
  // makes "I meant to replace that file" impossible. These are the rules of the dialog that
  // replaces it. Every one of them has a wrong version that type-checks.

  const collision = (over = {}) => ({
    source: '/a/main.rs',
    dest: '/b/main.rs',
    name: 'main.rs',
    merge: false,
    blocked: null,
    replaces: 1,
    keeps: 0,
    sample: [],
    truncated: false,
    ...over,
  })
  const folder = (over = {}) =>
    collision({ source: '/a/src', dest: '/b/src', name: 'src', merge: true, replaces: 12, keeps: 30, ...over })

  // The three answers, and which of them is real.
  ok(ask.canReplace(collision()), 'Replace is offered for a file onto a file')
  ok(
    !ask.canReplace(collision({ blocked: 'a folder and a file' })),
    'but NOT where one side is a folder and the other is not — swapping those is not a '
      + 'replacement of anything, and Rust refuses it whatever this says',
  )

  // A folder merge has to say so, in numbers, before it is chosen. "Replace src?" with no
  // counts is a dare rather than a question — and the word decides whether the files that are
  // only in the destination survive.
  ok(ask.askBody(folder()).includes('merges'), 'a folder Replace says it MERGES')
  ok(ask.askBody(folder()).includes('12 files'), 'and how many files it overwrites')
  ok(ask.askBody(folder()).includes('30 items'), 'and how many it leaves alone — the whole '
    + 'difference between merging and swapping the folder out')
  ok(
    ask.askBody(folder({ truncated: true })).includes('at least'),
    'a count that hit its walk budget is hedged: an exact-looking lower bound in a dialog '
      + 'about data loss is worse than no number',
  )
  ok(
    ask.askBody(collision()).includes('does not go to the trash'),
    'and a plain file replacement says the old one is simply gone',
  )
  eq(ask.askBody(collision({ blocked: 'that is a folder' })), 'that is a folder',
    'a blocked collision shows Rust\'s own sentence rather than inventing a second one')
  eq(ask.replaceLabel(folder()), 'Merge, replacing 12 files',
    'the button says what it does — "Replace" alone on a folder hides the merge again')
  eq(ask.replaceLabel(collision()), 'Replace', 'a file is just Replace')

  // Ten collisions must not be ten questions.
  const three = ask.startAsk([collision(), folder(), collision({ source: '/a/z.rs', name: 'z.rs' })])
  eq(ask.askProgress(three), '1 of 3', 'the position is shown when there is more than one')
  eq(ask.askProgress(ask.startAsk([collision()])), null, 'and not when there is only one')
  eq(ask.applyToRestLabel(three), 'Do the same for the remaining 2 files',
    'the checkbox names how many it covers — a user four questions into seven needs to know '
      + 'it is three, not seven')

  const one = ask.answerAsk(three, 'keepBoth', false)
  eq(ask.currentCollision(one).name, 'src', 'answering one moves to the next')
  ok(!ask.askIsDone(one), 'and the paste is not sent until every question has an answer')
  eq(ask.applyToRestLabel(ask.answerAsk(one, 'replace', false)), null,
    'the last question offers no "apply to the rest"')

  const all = ask.answerAsk(three, 'replace', true)
  ok(ask.askIsDone(all), 'apply-to-all answers every remaining collision at once')
  eq(
    ask.askDecisions(all),
    [
      { source: '/a/main.rs', choice: 'replace' },
      { source: '/a/src', choice: 'replace' },
      { source: '/a/z.rs', choice: 'replace' },
    ],
    'and each answer is bound to the source it answers for — a blanket `overwrite: true` flag '
      + 'would answer for paths the user was never asked about',
  )

  const mixed = ask.answerAsk(
    ask.startAsk([collision(), collision({ source: '/a/b', name: 'b', blocked: 'a file here, a folder there' })]),
    'replace',
    true,
  )
  eq(
    ask.askDecisions(mixed),
    [
      { source: '/a/main.rs', choice: 'replace' },
      { source: '/a/b', choice: 'keepBoth' },
    ],
    'Replace-to-all DOWNGRADES the one collision Replace is refused for, rather than sending a '
      + 'decision the backend rejects and failing the whole paste over it',
  )

  ok(
    ask.nothingWrittenYet().includes('Nothing has been written'),
    'the dialog can promise this because the questions are all asked before the paste is sent '
      + '— the version that asks as it writes can only apologise for the three files that landed',
  )
  ok(ask.cancelledNote().includes('Nothing was written'),
    'and the strip after a cancel says the same thing, because a dialog that vanishes in '
      + 'silence is indistinguishable from a paste that failed')

  /*
   * The collision NAME is not checked here, and that is deliberate.
   *
   * There used to be a `candidateName` in `clipboardModel.ts` — a second implementation of
   * `cide_fs::copy::candidate_name`, pinned row by row against the Rust source text by this
   * script. Nothing in the app ever called it. The panel cannot honestly predict `main copy.rs`
   * anyway: only Rust knows the destination's siblings, the folder may be collapsed, and the
   * name can be taken between the menu opening and the paste. What the user is told is the name
   * that actually landed (`PastedEntry.dest`, read by `pastedSummary` above), so the rule has
   * one implementation and `cide_fs::copy`'s own test
   * `a_collision_becomes_a_copy_rather_than_an_overwrite_or_a_refusal` pins the whole table.
   */

  // ---- reporting what Rust refused -------------------------------------------------------

  eq(errors.fsMessage({ kind: 'exists', detail: '/p/main.rs already exists' }),
    '/p/main.rs already exists',
    'a tuple variant carries its sentence in `detail` — this is the whole reason the strip '
      + 'stopped saying [object Object]')
  eq(errors.fsMessage({ kind: 'io', detail: { path: '/p/x', message: 'is not a directory' } }),
    '/p/x: is not a directory',
    'a struct variant is assembled in the order the Rust Display impl would')
  eq(errors.fsMessage({ kind: 'partialPaste', detail: { pasted: ['/p/a', '/p/b'], error: 'boom' } }),
    'boom — 2 path(s) had already been done',
    'a partial failure says how much had already happened; that is the whole reason the '
      + 'variant exists rather than a bare Io')
  eq(errors.fsMessage({ kind: 'noIndex' }), 'noIndex',
    'a variant with no content falls back to the tag, which is a word — that beats '
      + '[object Object] by the distance that matters')
  // The image-paste refusal (M35). Two claims, and the second is the load-bearing one: this
  // rejection reaches a bare Ctrl+V every time somebody presses it with text on the clipboard,
  // so a caller has to be able to tell it apart from a real failure and say nothing.
  eq(errors.fsMessage({ kind: 'noClipboardImage' }), 'the clipboard does not hold an image',
    'the one unit variant a user meets on purpose reads as a sentence, not as a tag')
  eq(errors.isNoClipboardImage({ kind: 'noClipboardImage' }), true,
    'matched on the tag, because prose is not an API')
  eq(errors.isNoClipboardImage({ kind: 'io', detail: { path: '/p/x', message: 'boom' } }), false,
    'a real failure must not be swallowed by the Ctrl+V path')
  eq(errors.isNoClipboardImage(new Error('boom')), false, 'nor a thrown Error')
  eq(errors.isNoClipboardImage(null), false, 'nor nothing at all')

  eq(errors.fsMessage('plain string'), 'plain string', 'a thrown string is already a message')
  eq(errors.fsMessage(new Error('boom')), 'boom', 'and a real Error has one')
  eq(errors.fsMessage(undefined), 'undefined', 'nothing thrown is still reported as something')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('file tree copy/cut/paste decisions: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
