/**
 * Checks `src/sidebar/treeStatus.ts` — the file tree's half of the git-status join.
 *
 * Same shape as `check-git-tree.mjs`, and for the same reason: this project has no JS test
 * runner, and adding one for two pure functions would be a larger commitment than the code it
 * tests. The Rust half is covered by `crates/cide-git/tests/tree_status.rs`, which drives real
 * temporary repositories. What is left over — and what is pinned here — is the behaviour that
 * exists only because the wire format is deliberately sparse:
 *
 *   * a row inside an untracked or ignored directory inherits it, because the backend
 *     deliberately does not enumerate what is inside one (that is the whole reason a fresh
 *     100k-file checkout does not put 100k entries on the wire),
 *   * a `modified` rollup mark on a folder is NOT inherited downwards — the bug that would
 *     paint every sibling of one edited file blue,
 *   * the resolver terminates on a malformed key rather than spinning inside a render,
 *   * directories take the colour and no letter.
 *
 * Run: `pnpm --dir ui run check:tree-status`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-tree-status-'))

/** `moveIndex`, named so the assertions below fit on one line each. */
const m2 = (c, key, at, count) => c.moveIndex(key, at, count)
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/treeStatus.ts',
      // The file tree's click rules, pinned at the foot of this file. They live in a module of
      // their own so a check script can hold them; see `clickSemantics.ts`.
      'src/sidebar/clickSemantics.ts',
      // Root/relative resolution for the context menu. Also its own module, and for a sharper
      // reason: the inline version of it read `TreeRow.root` — an *index* — as a path, which
      // type-checks and silently disables nothing and relativizes nothing. See `rowPaths.ts`.
      'src/sidebar/rowPaths.ts',
      '--outDir', out,
      // Pinned so the emitted layout is `<out>/sidebar/treeStatus.js` whether or not the
      // type-only import of `../ipc/generated` widens the inferred common root.
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The app's own tsconfig sets this, and it is what makes a map miss a `undefined` the
      // resolver has to handle rather than a `TreeStatus` it may assume.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'sidebar', 'treeStatus.js')}`)

  let failed = 0
  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }

  // The map a backend would produce for: `src/app.rs` edited, `src/new/` a whole new
  // untracked directory, `vendor/` ignored, `docs/gone.md` deleted from the working tree.
  // Note what the backend does NOT send: anything inside `src/new` or `vendor`.
  const R = '/w/app'
  const map = {
    [R]: 'modified',
    [`${R}/src`]: 'modified',
    [`${R}/src/app.rs`]: 'modified',
    [`${R}/src/new`]: 'untracked',
    [`${R}/docs`]: 'modified',
    [`${R}/vendor`]: 'ignored',
  }

  // --- exact hits ------------------------------------------------------------------------

  eq(m.statusAt(map, `${R}/src/app.rs`), 'modified', 'an exact hit is the status')
  eq(m.statusAt(map, `${R}/src`), 'modified', 'a rolled-up folder shows as modified')
  eq(m.statusAt(map, R), 'modified', 'so does the root row of a multi-root project')

  // --- the rollup mark is not inherited --------------------------------------------------

  eq(
    m.statusAt(map, `${R}/src/untouched.rs`),
    'clean',
    'a sibling of an edited file is clean — inheriting a `modified` rollup downwards would '
      + 'paint every file in the project blue, since the rollup reaches the root',
  )
  eq(
    m.statusAt(map, `${R}/docs/index.md`),
    'clean',
    'and so is a file beside a deleted one, even though its folder carries the deletion',
  )
  eq(m.statusAt(map, `${R}/README.md`), 'clean', 'a file directly under a marked root too')

  // --- container statuses ARE inherited --------------------------------------------------

  eq(
    m.statusAt(map, `${R}/src/new/mod.rs`),
    'untracked',
    'a file inside an untracked directory is untracked — the backend never enumerated it, so '
      + 'this is the only place that answer can come from',
  )
  eq(
    m.statusAt(map, `${R}/src/new/deep/a/b/c.rs`),
    'untracked',
    'however deep, and without an entry at any level in between',
  )
  eq(
    m.statusAt(map, `${R}/vendor/zlib/z.c`),
    'ignored',
    'the same rule carries `ignored`, which is reachable because the fs walk does not read '
      + '.gitignore files above a project root',
  )
  eq(
    m.statusAt(map, `${R}/src/new`),
    'untracked',
    'and the directory itself is still an exact hit, not an inheritance',
  )

  // --- outside the project ---------------------------------------------------------------

  eq(m.statusAt(map, '/w/other/x.rs'), 'clean', 'a path under no known ancestor is clean')
  eq(m.statusAt({}, `${R}/src/app.rs`), 'clean', 'an empty map — the first paint — is clean')
  eq(
    m.statusAt(m.NO_STATUS.statuses, `${R}/src/app.rs`),
    'clean',
    'and so is the placeholder the store holds before git has answered',
  )

  // --- termination -----------------------------------------------------------------------

  eq(m.statusAt(map, '/'), 'clean', 'the filesystem root terminates rather than looping')
  eq(m.statusAt(map, ''), 'clean', 'an empty key too')
  eq(m.statusAt(map, 'relative/path.rs'), 'clean', 'a key with no leading separator too')
  eq(
    m.statusAt(map, `${R}/${'a/'.repeat(200)}x.rs`),
    'clean',
    'a path deeper than the ancestor bound gives up and answers clean, rather than spinning '
      + 'inside a render',
  )
  eq(
    m.statusAt({ [`${R}/src`]: 'exploded' }, `${R}/src/x.rs`),
    'clean',
    'a status this build has never heard of is not treated as a container: an older webview '
      + 'against a newer backend under-tags rather than inheriting something it cannot draw',
  )

  // --- letters ---------------------------------------------------------------------------

  eq(m.letterFor('modified', false), 'M', 'M for a modified file')
  eq(m.letterFor('added', false), 'A', 'A for an added file')
  eq(m.letterFor('deleted', false), 'D', 'D for a deleted file')
  eq(m.letterFor('clean', false), '', 'a clean row draws nothing in the tag column')
  eq(
    m.letterFor('untracked', false),
    '',
    'untracked has colour and no letter — the mock draws no glyph for it and inventing a `?` '
      + 'would be a design decision made in a lookup table',
  )
  eq(m.letterFor('ignored', false), '', 'ignored likewise')
  eq(
    m.letterFor('modified', true),
    '',
    "a directory takes the colour and no letter, which is IDEA's treatment and keeps the tag "
      + 'column meaning "this file" rather than "something under here"',
  )
  eq(m.letterFor('untracked', true), '', 'every directory, whatever its status')
  eq(m.letterFor('exploded', false), '', 'an unknown status draws no letter rather than throwing')

  // --- clicks: select, and open only on the second one -----------------------------------

  /*
   * > *"In file tree when i do one click on element - we should select it, but not open the
   * > file. Open only by double click"*
   *
   * Pinned here rather than left in an `onMouseDown` because the interesting part is not the
   * happy path — it is what the *second* half of a double-click does. `click` fires twice,
   * with `detail` 1 and then 2, so a rule that toggled on both halves would expand a folder
   * and collapse it again, and one that opened on both would open the file twice.
   */
  const c = await import(`file://${join(out, 'sidebar', 'clickSemantics.js')}`)
  const click = (gesture, isDir, onTwisty = false) =>
    c.fileTreeClick({ gesture, isDir, onTwisty })
  const act = (select, toggle, open) => ({ select, toggle, open })

  eq(click('single', false), act(true, false, false), 'one click on a file selects and does '
    + 'not open it — the report this whole module exists for')
  eq(click('double', false), act(true, false, true), 'the second click opens it')
  eq(click('single', true), act(true, false, false), 'one click on a folder selects only too')
  eq(click('double', true), act(true, true, false), 'and the second expands it rather than '
    + 'opening a tab, because a folder has no tab to open')

  eq(
    click('single', true, true),
    act(true, true, false),
    'the twisty is exempt: one click on the ▸ folds the folder, because requiring a double '
      + 'click on an 11px arrow to do the only thing it does would be a worse tree',
  )
  eq(
    click('double', true, true),
    act(false, false, false),
    'and the second half of a double-click on the twisty does NOTHING — toggling on both '
      + 'halves would expand the folder and shut it again inside one gesture',
  )

  eq(c.gestureOf(1), 'single', '`detail` of 1 is a single click')
  eq(c.gestureOf(2), 'double', 'and 2 is the second of a pair — no timer, so no lag')

  eq(
    c.enterOn({ expandable: false }),
    act(true, false, true),
    'Enter always opens a leaf: the keyboard has no second click to wait for',
  )
  eq(c.enterOn({ expandable: true }), act(true, true, false), 'and expands a folder')

  // --- arrow movement --------------------------------------------------------------------

  eq(m2(c, 'ArrowDown', 0, 10), 1, 'Down moves one row')
  eq(m2(c, 'ArrowUp', 5, 10), 4, 'Up moves one row back')
  eq(m2(c, 'ArrowUp', 0, 10), 0, 'and stops at the top rather than wrapping to the bottom — a '
    + 'tree of 100 000 rows where overshooting the top costs you your place is unusable')
  eq(m2(c, 'ArrowDown', 9, 10), 9, 'the same at the end')
  eq(m2(c, 'Home', 7, 10), 0, 'Home is the first row')
  eq(m2(c, 'End', 2, 10), 9, 'End is the last')
  eq(m2(c, 'a', 2, 10), null, 'a key that is not ours answers null, so the handler can leave '
    + 'it to the browser rather than swallowing it')
  eq(m2(c, 'ArrowDown', 0, 0), null, 'an empty tree has nowhere to move')
  eq(m2(c, 'ArrowDown', 99, 10), 9, 'a stale index — the rows moved under the selection — is '
    + 'clamped rather than trusted')

  // --- rename and delete, the two keystrokes that act on the selected row ----------------

  /*
   * Both are focus-scoped rather than bound in `crates/cide-core/src/keymap.rs`, so nothing in
   * `check-key-gate.mjs` can see them: Delete in a terminal is a character the pty must get,
   * and Ctrl+R in a shell is readline's reverse-i-search. The rule lives in `clickSemantics.ts`
   * and is pinned here, which is the only place it *can* be pinned.
   *
   * The modifier rows are the point. `Delete` deletes and nothing else does; a modified Delete
   * is a different gesture in every file manager that has one — Shift+Delete is "permanently,
   * no trash" in Explorer and IDEA, and `fs_delete` has no such mode — so answering it with the
   * ordinary trash delete would do something other than what was asked, silently.
   */
  const mods = (held = {}) => ({
    ctrl: held.ctrl === true,
    meta: held.meta === true,
    alt: held.alt === true,
    shift: held.shift === true,
  })
  const act2 = (key, held) => c.treeKeyAction(key, mods(held))

  eq(act2('Delete'), 'delete', 'a bare Delete moves the selected row to the trash')
  eq(act2('Delete', { shift: true }), null, 'Shift+Delete is "permanently, no trash" elsewhere '
    + 'and cide has no such operation — so it is left alone rather than answered with the '
    + 'ordinary delete, which would do something other than what was asked')
  eq(act2('Delete', { ctrl: true }), null, 'and no other modifier claims it either')
  eq(act2('Delete', { alt: true }), null, 'Alt+Delete is not this gesture')
  eq(act2('Delete', { meta: true }), null, 'nor is ⌘Delete')

  eq(act2('r', { ctrl: true }), 'rename', 'Ctrl+R renames the selected row')
  eq(act2('R', { ctrl: true }), 'rename', 'and does so with Caps Lock on — `e.key` is the '
    + 'produced character, so the case is the user\'s keyboard state and not their intent')
  eq(act2('r', { meta: true }), 'rename', '⌘R is the same chord on macOS, where the default '
    + 'keymap rewrites every ctrl to meta')
  eq(act2('r', { ctrl: true, shift: true }), null, 'Ctrl+Shift+R stays free — it is "hard '
    + 'reload" muscle memory and belongs to nobody here yet')
  eq(act2('r', { ctrl: true, alt: true }), null, 'and Ctrl+Alt+R is not it')
  eq(act2('r'), null, 'a bare `r` types an `r`; the tree has no type-ahead to swallow it for')

  eq(act2('F2'), null, 'F2 is the rename key in Explorer and IDEA and is deliberately NOT '
    + 'claimed: the brief asked for Ctrl+R, and a second undocumented binding for one action '
    + 'is a decision rather than a freebie')
  eq(act2('Backspace'), null, 'Backspace is not Delete — on a Mac keyboard it is the key most '
    + 'people call Delete, which is exactly why answering it would be a surprise')
  eq(act2('ArrowDown'), null, 'navigation keys are not this function\'s business')

  // --- which root a row belongs to, and what "relative" means ----------------------------

  /*
   * The context menu's *Copy Relative Path*, *Rename…* and *Move to Trash* all hang off this.
   *
   * It shipped reading `TreeRow.root`, which is an index into `Project::roots` and not a path,
   * out of a `data-row-root` attribute — so the comparison was against the string `"0"`. Both
   * failures are invisible: *Copy Relative Path* produced the absolute path, which is exactly
   * what *Copy Path* one line above it produces, and the two root-refusing verbs stayed
   * enabled for Rust to reject. Pinned here so neither can come back quietly.
   */
  const rp = await import(`file://${join(out, 'sidebar', 'rowPaths.js')}`)
  const ROOTS = ['/home/u/work/cide', '/home/u/work/cide/ui']

  eq(rp.relativeTo('/home/u/work/cide/src/main.rs', ROOTS), 'src/main.rs',
    'a path under a root is copied relative to it, not absolute — an index read as a path '
      + 'made this item a duplicate of Copy Path')
  eq(
    rp.relativeTo('/home/u/work/cide/ui/src/App.tsx', ROOTS),
    'src/App.tsx',
    'the LONGEST root wins: with the first match, a nested root would name one file two '
      + 'different ways depending on the order the roots happen to be in',
  )
  eq(rp.relativeTo('/home/u/work/cide-old/x.rs', ROOTS), '/home/u/work/cide-old/x.rs',
    'a sibling directory whose name merely starts with a root is NOT inside it — a bare '
      + 'startsWith would slice it at the wrong offset and produce `ld/x.rs`')
  eq(rp.relativeTo('/elsewhere/x.rs', ROOTS), '/elsewhere/x.rs',
    'a path under no root keeps its absolute form rather than being relativized to nothing')
  eq(rp.relativeTo('/home/u/work/cide', ROOTS), '/home/u/work/cide',
    'a root itself is absolute too: relative to itself it is the empty string, and an empty '
      + 'clipboard is indistinguishable from a menu item that did nothing')
  eq(rp.relativeTo('/home/u/work/cide/src/main.rs', []), '/home/u/work/cide/src/main.rs',
    'and with no roots at all — the panel before the bootstrap lands')

  eq(rp.isRootPath('/home/u/work/cide', ROOTS), true,
    'a project root is a root: Rename and Move to Trash are disabled with a reason, because '
      + '`check_not_root` refuses them and a refusal nobody sees looks like a dead control')
  eq(rp.isRootPath('/home/u/work/cide/ui', ROOTS), true, 'so is a second, nested one')
  eq(rp.isRootPath('/home/u/work/cide/ui/src', ROOTS), false, 'a directory inside one is not')
  eq(rp.isRootPath('/home/u/work/cide/src/main.rs', ROOTS), false, 'nor is a file')
  eq(rp.rootOf('/home/u/work/cide/src/main.rs', ROOTS), '/home/u/work/cide',
    'and the root a row resolves to is a path, which is the whole point')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('file tree status: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
