/**
 * Checks `src/sidebar/newEntry.ts` — where a new file goes, and whether its name can be one.
 *
 * Same shape as `check-tree-status.mjs` and `check-rows.mjs`, and for the same reason: this
 * project has no JS test runner, and adding one for two pure functions would be a larger
 * commitment than the code it tests.
 *
 * What is pinned here is every edge the feature actually turns on, because each of them is
 * invisible in a screenshot and each has a wrong answer that type-checks:
 *
 *   * a **file** row means its *parent*, not the file — the single most load-bearing rule, and
 *     the one whose wrong version ("inside the thing I clicked") has no meaning at all;
 *   * a **folder** row means that folder;
 *   * **no row** means the project's first root, and a multi-root project therefore has to be
 *     able to *say which* — `label`, and `atTop` for the single-root project whose root the
 *     tree does not draw;
 *   * every name rule: `/`, a leading dot, empty, whitespace-only, a duplicate, `.`/`..`, and
 *     a path that would escape the root;
 *   * that a leading dot is a *note* and not a refusal, since `.gitignore` is the most likely
 *     thing anybody creates from this menu.
 *
 * `cide_fs::ops::check_name` holds the same list on the Rust side. Two copies on purpose: this
 * one tells the user while they type, that one is what a name arriving over IPC has to pass.
 *
 * Run: `pnpm --dir ui run check:new-entry`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-new-entry-'))

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/newEntry.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The app's own tsconfig sets this. It is what makes `roots[0]` a `string | undefined`
      // rather than a `string`, which is the difference between `targetFor(null, [])`
      // answering `null` and answering a target whose parent is the string "undefined".
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'sidebar', 'newEntry.js')}`)

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

  // ---- where a new entry goes -----------------------------------------------------------

  const ONE = ['/home/u/work/cide']
  const TWO = ['/home/u/work/cide', '/home/u/work/other']

  eq(
    m.targetFor({ path: '/home/u/work/cide/src/main.rs', isDir: false }, ONE)?.parent,
    '/home/u/work/cide/src',
    'a FILE row creates beside the file, in its parent — not "inside" a file, which means '
      + 'nothing, and not at the root, which is where a naive implementation puts it',
  )
  eq(
    m.targetFor({ path: '/home/u/work/cide/src', isDir: true }, ONE)?.parent,
    '/home/u/work/cide/src',
    'a FOLDER row creates inside that folder',
  )
  eq(
    m.targetFor({ path: '/home/u/work/cide/main.rs', isDir: false }, ONE)?.parent,
    '/home/u/work/cide',
    'a file directly under the root resolves to the root, not to the empty string',
  )
  eq(m.targetFor({ path: '/a.rs', isDir: false }, ['/'])?.parent, '/',
    'a file at the filesystem root has "/" as its parent, not "" — an empty parent would '
      + 'reach Rust as a relative path and be refused, which is a correct answer to a '
      + 'question nobody asked')

  eq(m.targetFor(null, ONE)?.parent, '/home/u/work/cide',
    'no row — the empty space under the last row — means the project root')
  eq(m.targetFor(null, ONE)?.atTop, true,
    'a single-root project draws no row for its root (Index::show_roots), so the draft row '
      + 'belongs at the top of the list rather than under an anchor that does not exist')
  eq(m.targetFor(null, TWO)?.parent, '/home/u/work/cide',
    'a multi-root project uses the FIRST root, deterministically — there is no "current" '
      + 'root, and inventing one would put files somewhere that depends on the scroll')
  eq(m.targetFor(null, TWO)?.label, 'cide',
    'and the label names which one, so the menu item can say so instead of picking silently')
  eq(m.targetFor(null, TWO)?.atTop, false,
    'with several roots each root IS a row, so the draft anchors under it like any folder')
  eq(m.targetFor(null, []), null,
    'a project with no roots — the panel before the bootstrap lands — has nowhere to create '
      + 'anything, and the menu items are simply absent')

  eq(
    m.targetFor({ path: '/home/u/work/cide', isDir: true }, ONE)?.atTop,
    true,
    'right-clicking the root row of a single-root project is the same top-of-list case',
  )
  eq(
    m.targetFor({ path: '/home/u/work/cide', isDir: true }, TWO)?.atTop,
    false,
    'the same row in a multi-root project is a real row and anchors under itself',
  )
  eq(
    m.targetFor({ path: '/home/u/work/cide/ui', isDir: true }, ONE)?.atTop,
    false,
    'an ordinary folder is never the top-of-list case, however shallow it is',
  )

  // Two roots that share a basename is the ordinary monorepo shape, and a label that names
  // both of them `src` is worse than a long one.
  const SAME = ['/home/u/a/src', '/home/u/b/src']
  eq(m.targetFor(null, SAME)?.label, '/home/u/a/src',
    'two roots with the same basename fall back to the full path, because "New File in src" '
      + 'in a project with two directories called src tells the user nothing')
  eq(m.targetFor({ path: '/home/u/a/src/x', isDir: true }, SAME)?.label, 'x',
    'the clash rule applies to roots only — an ordinary folder keeps its short name')

  // ---- what a name may be ----------------------------------------------------------------

  const SIBS = ['main.rs', 'lib.rs', 'README.md']
  const err = (name, siblings = SIBS, dir = false) => m.checkName(name, siblings, dir).error
  const note = (name, siblings = SIBS, dir = false) => m.checkName(name, siblings, dir).note

  ok(err('') !== null, 'an empty name cannot be created')
  ok(err('   ') !== null, 'nor a name that is only whitespace')
  ok(err('\t') !== null, 'nor one that is only a tab')
  eq(err('   '), err(''),
    'whitespace-only and empty give the SAME message: to the user they are one mistake, and '
      + 'a message about "whitespace" is a message about only one of them')

  ok(err('src/main.rs') !== null,
    'a "/" is refused with a reason rather than obeyed as two components or rewritten to '
      + '"src_main.rs" the way the rename box does it — rename is fixing a typo in a name, '
      + 'this is choosing a location, and `src/main.rs` is plainly what was meant')
  ok(err('src/main.rs').includes('/'),
    'and the message names the character, so the user can see what to delete')
  ok(err('a\0b') !== null, 'a NUL character — unreachable from a keyboard, reachable from a paste')

  ok(err('.') !== null, '"." already names this folder')
  ok(err('..') !== null, '".." already names the folder above it, which is also how a path '
    + 'escapes the project root from a name box')
  ok(err('...') === null, 'three dots is a legal filename, unlike one or two')

  ok(err('main.rs') !== null, 'a name that already exists is refused BEFORE the gesture, not '
    + 'after it — "the OS will reject it" arrives too late to be an answer')
  ok(err('main.rs').includes('main.rs'), 'and the message says which name')
  ok(err('main.rs ') !== null,
    'the duplicate check runs on the TRIMMED name, so a trailing space does not sneak a '
      + 'second "main.rs " past it')
  ok(err('Main.rs') === null,
    'a name differing only in case is legal on Linux and is NOT refused')
  ok(note('Main.rs') !== null,
    'but it is worth saying, because a case-insensitive filesystem will treat it as the same '
      + 'file and the refusal would then arrive from Rust with no warning')

  ok(err('.gitignore') === null,
    'a LEADING DOT is allowed. It is a legitimate filename and the single most likely thing '
      + 'anybody creates from this menu; refusing it would be inventing a rule the '
      + 'filesystem does not have')
  ok(note('.gitignore') !== null,
    'it does get a note: a dot-file may be hidden by the project ignore rules, so the row '
      + 'may not appear — which the panel says again afterwards if it really did not')
  eq(note('main.rs', ['other.rs']), null, 'an ordinary name gets neither an error nor a note')

  // ---- a name that is still only the seed --------------------------------------------------
  //
  // The file tree's *New ▸ Rust* opens the box holding `.rs` with the caret in front of it, so
  // this is the state EVERY such gesture starts in — and the one arm above it would otherwise
  // fall into is the dot-file NOTE, which is not a refusal at all. Enter would then create a
  // hidden file literally called `.rs`, carrying a warning about ignore rules for a file nobody
  // had finished naming. The rule lives in `checkName` rather than in the panel exactly so that
  // it can be driven from here; in a `useCallback` it would be pinned by a source grep and
  // nothing else.

  const seeded = (name, seed) => m.checkName(name, SIBS, false, seed)

  ok(seeded('.rs', '.rs').error !== null,
    'a box holding nothing but the seed is refused, like the empty box it stands in for')
  ok(seeded('.rs', '.rs').error.includes('.rs'),
    'and the message names the seed, because "type a name" alone does not say where')
  ok(seeded('  .rs  ', '.rs').error !== null,
    'compared TRIMMED, because `nameToSend` trims: one trailing space would otherwise walk '
      + 'straight past this and create the file anyway')
  eq(m.nameToSend('.rs', SIBS, false, '.rs'), null, 'so nothing is sent for it')

  eq(seeded('parser.rs', '.rs').error, null,
    'a real name ending in the seed is NOT the seed — which is the whole point of the gesture')
  eq(seeded('parser.rs', '.rs').note, null, 'and carries no note either')
  ok(seeded('.gitignore', '.rs').error === null,
    'nor does a different dot-file, which keeps its own note and its own meaning')
  ok(seeded('.gitignore', '.rs').note !== null, '…that note')

  ok(m.checkName('.rs', SIBS, false).error === null,
    'and with NO seed — what a plain *New File…* passes — `.rs` is an ordinary dot-file again, '
      + 'so the unseeded box behaves exactly as it did before M64')
  ok(m.checkName('.rs', SIBS, false).note !== null, '…with its dot-file note')
  eq(m.nameToSend('.rs', SIBS, false), '.rs', 'and is sent')

  ok(seeded('.excalidraw', '.excalidraw').error !== null,
    'the drawing seed is refused for a sharper reason than the rest: `drawingKindFor` wants a '
      + 'stem in front of the suffix, so a file called exactly `.excalidraw` would not even '
      + 'open in the pane the gesture promised')

  // ---- what is actually sent -------------------------------------------------------------

  eq(m.nameToSend('  main2.rs  ', SIBS, false), 'main2.rs',
    'the name is trimmed on the way out, by the SAME function that checked it — when those '
      + 'were two expressions, a name with a trailing space passed the sibling check in its '
      + 'trimmed form and was then created untrimmed')
  eq(m.nameToSend('main.rs', SIBS, false), null, 'a refused name is never sent')
  eq(m.nameToSend('', SIBS, false), null, 'and neither is an empty one')
  eq(m.nameToSend('../escaped.rs', SIBS, false), null,
    'a name that would climb out of the project is refused here as well as in Rust')

  // The message is phrased for the thing being made, so a folder gesture never says "file".
  ok(m.checkName('', [], true).error.includes('folder'), 'an empty FOLDER name says folder')
  ok(m.checkName('', [], false).error.includes('file'), 'an empty FILE name says file')

  eq(m.basenameOf('/home/u/work/cide'), 'cide', 'basenameOf is the label the menu shows')
  eq(m.basenameOf('/'), '/', 'and it does not answer the empty string for the root')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('new file/folder placement and naming: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
