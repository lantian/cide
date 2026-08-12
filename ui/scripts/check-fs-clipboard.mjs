/**
 * Checks `src/sidebar/clipboardModel.ts` and `src/sidebar/fsError.ts` — every decision the
 * file tree's Copy / Cut / Paste makes before it calls Rust, and how it reports what Rust
 * says back.
 *
 * Same shape as `check-new-entry.mjs` beside it, and for the same reason: this project has no
 * JS test runner, and the rules worth pinning are pure functions whose wrong versions all
 * type-check. Each of these is invisible in a screenshot and expensive to get wrong:
 *
 *   * **where a paste lands** — a file row means its *parent*, exactly as *New File…* does,
 *     because two rules that agree today and are written twice disagree after the next change;
 *   * **the collision name**, which has a second copy in `cide_fs::copy::candidate_name`. The
 *     table below is asserted against the Rust source text as well as against this module, so
 *     the panel cannot promise `main copy.rs` while Rust writes something else;
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
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
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

  eq(m.clipboardText(['/a/x.rs', '/a/y.rs']), '/a/x.rs\n/a/y.rs',
    'Copy also puts the paths on the SYSTEM clipboard, one per line, so a copy in the tree '
      + 'is usable in a terminal pane — cide does not put file references there, and cannot')

  // ---- what is said after a paste --------------------------------------------------------

  const pasted = (over) => ({ source: '/a/main.rs', dest: '/b/main.rs', renamed: false, skipped: 0, ...over })

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

  // ---- the collision name, against Rust --------------------------------------------------

  const TABLE = [
    ['main.rs', 0, false, 'main.rs'],
    ['main.rs', 1, false, 'main copy.rs'],
    ['main.rs', 2, false, 'main copy 2.rs'],
    ['a.tar.gz', 1, false, 'a.tar copy.gz'],
    ['.gitignore', 1, false, '.gitignore copy'],
    ['Makefile', 1, false, 'Makefile copy'],
    ['foo.bar', 1, true, 'foo.bar copy'],
    ['src', 1, true, 'src copy'],
  ]
  for (const [name, attempt, directory, expected] of TABLE) {
    eq(m.candidateName(name, attempt, directory), expected,
      `candidateName(${JSON.stringify(name)}, ${attempt}, ${directory})`)
  }

  /*
   * The same table, asserted against the Rust test that pins the other copy.
   *
   * Two implementations of one rule is a deliberate choice — this one tells the user what the
   * copy will be called while they can still change their mind, and `cide_fs::copy` is the one
   * that atomically claims the name against a directory that can gain it in between. What is
   * not deliberate is them drifting, and nothing else would catch it: the panel would promise
   * `main copy.rs`, Rust would write something else, and every test on both sides would pass.
   *
   * Whitespace *between tokens* is stripped from both sides, so rustfmt wrapping an assertion
   * across lines cannot fail this — but whitespace *inside a string literal* is kept, because
   * the space in `main copy.rs` is the very thing being pinned.
   */
  const squash = (source) =>
    source
      .split(/("(?:[^"\\]|\\.)*")/)
      .map((part, i) => (i % 2 === 1 ? part : part.replace(/\s+/g, '')))
      .join('')

  const rust = squash(readFileSync('../crates/cide-fs/src/copy.rs', 'utf8'))
  for (const [name, attempt, directory, expected] of TABLE) {
    const needle = squash(`candidate_name("${name}", ${attempt}, ${directory}), "${expected}"`)
    ok(rust.includes(needle),
      `the Rust side pins the same case: candidate_name(${JSON.stringify(name)}, ${attempt}, `
        + `${directory}) == ${JSON.stringify(expected)}`)
  }

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
