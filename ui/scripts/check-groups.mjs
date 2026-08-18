/**
 * Checks `src/sidebar/groupRows.ts` and `src/sidebar/rowPaths.ts` — what a file-tree row is
 * allowed to do now that not every row is a file.
 *
 * # What this is defending
 *
 * Until M13 the tree had two kinds and one question answered everything: `kind === 'dir'`. The
 * *External Libraries* group put four things into that list at once —
 *
 *   * a **header** that folds and does nothing else, with no path on disk;
 *   * a **note**, which is a sentence and not a row about a file at all;
 *   * ordinary `dir`/`file` rows that live **outside every project root**, where
 *     `cide_fs::ops::check_within` refuses Rename, Cut, Paste and Move to Trash;
 *   * and a second dim column (`TreeRow.detail`) that changes *without any row moving*.
 *
 * Every one of those has a wrong version that type-checks and is invisible in a screenshot: a
 * group header that opens a tab, a note that can be renamed, a *Move to Trash* on `serde` that
 * is enabled and always errors, `New File in cide…` offered from a right-click on a crate. The
 * rules therefore live in a pure module and are asserted here rather than in an `onMouseDown`,
 * which is the one place no check script can compile — the lesson this project has paid for
 * four times.
 *
 * The tail reads `FileTree.tsx`, `treeStore.ts` and `Explorer.tsx` as *source*, because the
 * wiring cannot be executed here (no DOM, no Tauri). That half is what stops the rules being
 * correct and unreachable, which is this repository's signature defect.
 *
 * Run: `pnpm --dir ui run check:groups`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-groups-'))
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

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/groupRows.ts',
      'src/sidebar/rowPaths.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const g = await import(`file://${join(out, 'sidebar', 'groupRows.js')}`)
  const rp = await import(`file://${join(out, 'sidebar', 'rowPaths.js')}`)

  // --- the sentinel a header carries -----------------------------------------------------

  /*
   * It has to be **non-absolute**, and that is not cosmetic. `cide_fs::ops::check_within`
   * refuses a non-absolute path outright, so every file command in `cmd::fs` already declines a
   * group header without a special case; `rowPaths.mutationRefusal` keys on the same shape, so
   * the menu and the handler cannot drift into two rules.
   */
  const HEADER = g.groupPath(g.EXTERNAL_LIBRARIES)
  eq(HEADER, 'cide://group/externalLibraries', 'the header path is the Rust sentinel verbatim')
  ok(!HEADER.startsWith('/'), 'a header path is not absolute, so `check_within` refuses it')
  eq(g.groupIdOf(HEADER), g.EXTERNAL_LIBRARIES, 'the id round-trips out of the path')
  eq(g.groupIdOf('/home/u/work/cide/src/main.rs'), null, 'a real path holds no group id')
  eq(
    g.EXTERNAL_LIBRARIES,
    'externalLibraries',
    'the id matches `cide_app::libraries::GROUP_ID`; a rename here silently orphans the command',
  )

  ok(g.isSyntheticPath(HEADER), 'a header is synthetic')
  ok(g.isSyntheticPath('cide://note/cargo is not on PATH'), 'so is a note')
  ok(!g.isSyntheticPath('/home/u/.cargo/registry/src/x/serde-1.0.229/src/lib.rs'), 'a crate source is not')
  ok(!g.isSyntheticPath('/home/u/work/cide/src/main.rs'), 'nor is a project file')

  // --- what each kind lets the user do ---------------------------------------------------

  const verbs = (kind, inProject = true) => {
    const v = g.rowVerbs(kind, inProject)
    return [v.expandable, v.openable, v.actionable, v.mutable, v.addressable]
  }

  eq(
    verbs('group'),
    [true, false, false, false, false],
    'a group header folds and does NOTHING else: no tab, no clipboard, no rename, no reveal',
  )
  eq(
    verbs('note'),
    [false, false, false, false, false],
    'a note is a sentence — every verb is off, including the twisty',
  )
  /*
   * The pin — *Project Notes* — is the header's mirror image: it opens and does nothing else.
   * Each of the four `false`s is a decision. `expandable` because Rust refuses to mark a pin
   * expanded, so a twisty would fold nothing; `actionable` because Ctrl+X on the cursor must not
   * put `cide://group/projectNotes` on the clipboard; `addressable` because *Copy Path* would
   * copy that same sentinel — a correct-looking answer about the wrong thing.
   */
  eq(
    verbs('pin'),
    [false, true, false, false, false],
    'a pin OPENS and does nothing else: no twisty, no clipboard, no rename, no Copy Path',
  )
  eq(
    verbs('pin', false),
    [false, true, false, false, false],
    'and its verbs do not depend on containment — it has no path on disk for that to be a ' +
      'question about',
  )
  eq(verbs('dir'), [true, false, true, true, true], 'a project directory keeps everything it had')
  eq(verbs('file'), [false, true, true, true, true], 'and so does a project file')

  /*
   * The population that made this module necessary. A dependency source is a perfectly ordinary
   * `file` row — it opens, it can be copied, its path can be copied — and the *disk-changing*
   * verbs are off, because `check_within` refuses them and an enabled item that always errors is
   * the dead control this panel has already shipped twice.
   */
  eq(
    verbs('file', false),
    [false, true, true, false, true],
    'a dependency source opens and is copyable, and is NOT renamed, cut or trashed',
  )
  eq(
    verbs('dir', false),
    [true, false, true, false, true],
    'the same for a directory inside one',
  )
  ok(
    g.rowVerbs('file', false).openable,
    'opening a library file is deliberately allowed — the tab is read-only, and Go to '
      + 'definition has opened these files since M12',
  )
  eq(
    verbs('sideways'),
    [false, false, false, false, false],
    'a kind this build has never heard of draws as an inert row rather than throwing inside a '
      + 'render, which would unmount the whole tree',
  )

  // --- icons ------------------------------------------------------------------------------

  eq(
    g.groupIcon('group', false, g.EXTERNAL_LIBRARIES),
    'folder-lib',
    'a folded External Libraries header draws the library folder',
  )
  eq(
    g.groupIcon('group', true, g.EXTERNAL_LIBRARIES),
    'folder-lib-open',
    'and an open one draws the open variant',
  )
  /*
   * M13's second group takes a *different* folder, and that is the point of the id parameter:
   * with one stem for every header the two groups would look like two halves of one thing.
   */
  eq(
    g.groupIcon('group', false, g.SCRATCHES),
    'folder-temp',
    'Scratches draws the temporary-files folder, not the library one',
  )
  eq(g.groupIcon('group', true, g.SCRATCHES), 'folder-temp-open', 'and its open variant')
  eq(
    g.groupIcon('group', false, 'a-group-this-build-has-never-heard-of'),
    'folder',
    'an unknown id falls back to the plain folder — an older webview against a newer backend ' +
      'draws a generic row rather than a 404 and a blank one',
  )
  /*
   * A pin draws the glyph of the file it opens, not a folder, and it has no open variant — it
   * never folds, so a `-open` suffix would name an icon nothing ever draws.
   */
  eq(
    g.groupIcon('pin', false, g.PROJECT_NOTES),
    'markdown',
    'Project Notes draws the markdown glyph, so the row looks like the tab it produces',
  )
  eq(
    g.groupIcon('pin', true, g.PROJECT_NOTES),
    'markdown',
    'and a pin has no open variant, because it never folds',
  )
  eq(
    g.groupIcon('pin', false, 'a-pin-this-build-has-never-heard-of'),
    'document',
    'an unknown pin falls back to a document rather than a folder: a pin stands for one file',
  )
  eq(
    g.groupIcon('note', false, null),
    null,
    'a note draws no icon — a document glyph would make the sentence read as a file name',
  )
  eq(
    g.groupIcon('file', false, null),
    null,
    'an ordinary row goes through the icon theme, not here',
  )

  // --- why a mutation is refused, in the words the menu greys the row with -----------------

  const ROOTS = ['/home/u/work/cide', '/home/u/work/cide/ui']
  const CRATE = '/home/u/.cargo/registry/src/index.crates.io-1949/serde-1.0.229/src/lib.rs'

  eq(rp.mutationRefusal([], ROOTS), null, 'an empty set refuses nothing')
  eq(rp.mutationRefusal(['/home/u/work/cide/src/main.rs'], ROOTS), null, 'a project file is fine')
  eq(rp.mutationRefusal(['/home/u/work/cide/ui/src/App.tsx'], ROOTS), null, 'a nested root too')
  ok(
    rp.mutationRefusal([HEADER], ROOTS)?.includes('heading'),
    'a group header is refused as a heading rather than as an out-of-project path — the two ' +
      'send the user looking in different places',
  )
  ok(
    rp.mutationRefusal([CRATE], ROOTS)?.includes('outside the project'),
    'a dependency source is refused for being outside the project',
  )
  ok(
    rp.mutationRefusal(['/home/u/work/cide/src/main.rs', CRATE], ROOTS) !== null,
    'ONE bad path in a multi-row selection refuses the whole set — `fs_delete` would refuse it ' +
      'halfway through otherwise, having already trashed the good ones',
  )
  eq(
    rp.mutationRefusal(['/home/u/work/cide/src/main.rs'], []),
    'This file is outside the project, so cide will not move, rename or delete it',
    'with no roots at all — the panel before the bootstrap lands — every path is outside',
  )
  ok(
    rp.mutationRefusal(['/home/u/work/cide-old/x.rs'], ROOTS) !== null,
    'a sibling directory whose name merely starts with a root is not inside it',
  )

  // --- and that the rules are actually wired ----------------------------------------------
  //
  // Everything above proves the rules are right. This half proves something calls them, which
  // is the half this project keeps getting wrong: fourteen features have shipped complete,
  // correct and reachable from nothing.

  const tree = readFileSync('src/sidebar/FileTree.tsx', 'utf8')
  const store = readFileSync('src/sidebar/treeStore.ts', 'utf8')
  const dispatch = readFileSync('src/keys/dispatch.ts', 'utf8')
  const commands = readFileSync('../crates/cide-core/src/commands.rs', 'utf8')

  ok(
    /rowVerbs\(row\.kind, true\)/.test(tree),
    '`FileTree` asks `rowVerbs` rather than `kind === \'dir\'` — the six-call-site question ' +
      'this module exists to answer once',
  )
  ok(
    !/action\.toggle && row\.kind === 'dir'/.test(tree),
    'the click handler no longer folds on `kind === \'dir\'`, which would leave a group header ' +
      'with a twisty that does nothing',
  )
  ok(
    /const hasTwisty = verbs\.expandable && row\.hasChildren/.test(tree),
    'the twisty is drawn from `expandable`, so a group header gets one',
  )
  ok(
    /rowVerbs\(row\.kind, true\)\.expandable/.test(store),
    '`treeStore.toggle` gates on `expandable` — without it `fs_expand` is never called for a ' +
      'group and the header is a twisty that does nothing',
  )
  ok(
    /mutationRefusal/.test(tree),
    '`FileTree` consults `mutationRefusal`, or every refused verb is an enabled menu item',
  )
  // Five call sites, one per verb the rule governs. Counted rather than named, because naming
  // them would pin the shape of the menu rather than the rule.
  ok(
    (tree.match(/mutationRefusal\(/g) ?? []).length >= 5,
    'every verb that changes the disk — rename, delete, cut, paste, create — asks it',
  )
  ok(
    /row\.kind === 'note'\) return \[\]/.test(tree),
    'right-clicking a note opens no menu at all: every item in it is about a file',
  )
  ok(
    /if \(row\.kind === 'group'\) \{/.test(tree),
    'and a group header gets its own one-item menu rather than fourteen greyed rows',
  )
  ok(
    /detail: true,/.test(store),
    '`ROW_FIELDS` names `detail`, or `sameRows` calls two rows identical when a resolution ' +
      'lands and the count never appears',
  )
  ok(
    /styles\.detail/.test(tree) && /row\.detail !== null/.test(tree),
    'and the tree actually draws it — a field on the wire that no renderer reads is instance ' +
      'fifteen of this project\'s recurring defect',
  )

  // The palette route. The group is the last row of a virtualized tree, so without a command it
  // is drawn and, on any repository with depth, unreachable.
  ok(
    /Command::new\("view\.externalLibraries"/.test(commands),
    '`view.externalLibraries` is in the Rust registry',
  )
  ok(
    /case 'view\.externalLibraries':/.test(dispatch),
    'and `dispatch.ts` handles it — `check-commands.mjs` enforces this too, from the other side',
  )
  ok(
    /revealGroup\(EXTERNAL_LIBRARIES\)/.test(dispatch),
    'the handler reveals the group by the id this module exports, not by a second spelling',
  )
  ok(
    /revealGroup\(id\)/.test(store) || /async revealGroup/.test(store),
    '`treeStore` implements `revealGroup`',
  )
  ok(
    /if \(shown\) return/.test(dispatch) && /has no External Libraries group/.test(dispatch),
    'and it REPORTS when there is no group: a project in another language has none, and a ' +
      'palette row that scrolls nowhere and says nothing is the defect this file is about',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('file tree groups: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
