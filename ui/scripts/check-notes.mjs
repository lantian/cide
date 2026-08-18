/**
 * *Project Notes* — every link in the chain from the pinned row to a saved markdown buffer.
 *
 * # What this is defending
 *
 * The feature is one **pinned** file-tree row that opens one file per project. Almost all of it
 * is a rule rather than a mechanism, and every one of those rules lives somewhere a screenshot
 * cannot see:
 *
 *   * the row is a new `TreeRowKind`, and the frontend's copy of that union is hand-maintained
 *     (`groupRows.ts` is import-free so `check:groups` can compile it standalone) — so the two
 *     spellings of the set are compared here as text, which is the only thing keeping them in
 *     step;
 *   * the row is **openable and nothing else**: a pin that were `actionable` would put
 *     `cide://group/projectNotes` on the clipboard, and one that were `addressable` would put it
 *     behind *Copy Path* — a correct-looking answer about a string that names no file;
 *   * the id, the label and the command id cross the wire as bare strings that nothing
 *     type-checks: a rename on either side silently orphans the row or the command;
 *   * the file is created by `fs_notes_ensure` and **never truncated**, which is the one line in
 *     the feature that can destroy a user's writing;
 *   * and the whole thing is reachable by exactly one double-click, one menu item and one
 *     palette row, which is where this project's dead features have always died.
 *
 * The pure half — the row rules — is compiled and driven. The wiring half is asserted in source,
 * because those bugs are *missing calls* rather than wrong functions and no module's own tests
 * can see one. The Rust half (where the file lives, the keying, the create-without-truncating,
 * the pin's draw order) is covered by `cide_core::notes`, `cide_fs::groups` and `cide_app::notes`
 * tests; this file pins that the two sides agree on the strings that cross between them.
 *
 * What no check can prove, and which needs `./run.sh`: that the tab CodeMirror mounts really has
 * markdown highlighting, a working dirty marker, Ctrl+S, undo and find-in-file. This proves the
 * routing and the strings agree, not that the right element received them.
 *
 * Run: `pnpm --dir ui run check:notes`
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-notes-'))
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

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8')

/**
 * Source with comments removed.
 *
 * A grep over raw source matches the *explanation* of a rule as happily as the rule, so an
 * assertion written that way stays green after the code is deleted and only the prose is left —
 * and this feature's comments quote nearly every symbol asserted below. `check-scratch.mjs` and
 * `check-paths.mjs` strip for the same reason. String-aware, so a `'//'` inside a literal does
 * not eat the rest of the line.
 */
const stripComments = (source) => {
  let out = ''
  let i = 0
  while (i < source.length) {
    const ch = source[i]
    if (ch === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++
      continue
    }
    if (ch === '/' && source[i + 1] === '*') {
      i += 2
      while (i < source.length && !(source[i] === '*' && source[i + 1] === '/')) i++
      i += 2
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch
      out += ch
      i++
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') {
          out += source[i]
          i++
        }
        if (i < source.length) {
          out += source[i]
          i++
        }
      }
      out += quote
      i++
      continue
    }
    out += ch
    i++
  }
  return out
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

  // The sources that have to agree on strings nobody type-checks.
  const notesRs = read('../../crates/cide-app/src/notes.rs')
  const coreNotes = read('../../crates/cide-core/src/notes.rs')
  const appGroups = read('../../crates/cide-app/src/groups.rs')
  const fsGroups = read('../../crates/cide-fs/src/groups.rs')
  const commandsRs = read('../../crates/cide-core/src/commands.rs')
  const cmdFs = read('../../crates/cide-app/src/cmd/fs.rs')
  const contract = read('../../contract/commands.json')
  const generated = read('../src/ipc/generated.ts')
  const client = read('../src/ipc/client.ts')
  const dispatch = read('../src/keys/dispatch.ts')
  const tree = read('../src/sidebar/FileTree.tsx')
  const explorer = read('../src/sidebar/Explorer.tsx')
  const app = read('../src/App.tsx')
  const rowsSrc = read('../src/sidebar/groupRows.ts')

  // --- the sentinel the pinned row carries -------------------------------------------------

  /*
   * The same scheme a group header uses, and for the same reason: the path must be **not
   * absolute**, so `cide_fs::ops::check_within` refuses it outright and every file command in
   * `cmd::fs` declines it by a check that already exists. It also makes the create-before-open
   * step impossible to forget — a handler that passed the sentinel to `file.open` fails loudly
   * instead of quietly opening the wrong thing.
   */
  const PIN = g.groupPath(g.PROJECT_NOTES)
  eq(g.PROJECT_NOTES, 'projectNotes', 'the id is the literal Rust one')
  eq(PIN, 'cide://group/projectNotes', 'the pinned row carries the Rust sentinel verbatim')
  ok(!PIN.startsWith('/'), 'and it is not absolute, so `check_within` refuses it')
  eq(g.groupIdOf(PIN), g.PROJECT_NOTES, 'the id round-trips out of the path')
  ok(g.isSyntheticPath(PIN), 'a pin is synthetic')

  // --- what the row lets the user do -------------------------------------------------------

  const verbs = (kind, inProject = true) => {
    const v = g.rowVerbs(kind, inProject)
    return [v.expandable, v.openable, v.actionable, v.mutable, v.addressable]
  }
  eq(
    verbs('pin'),
    [false, true, false, false, false],
    'a pin OPENS and does nothing else — the header is expand-only and this is its mirror image',
  )
  eq(
    verbs('pin', false),
    [false, true, false, false, false],
    'and containment is not a question about a row with no path on disk',
  )
  ok(
    !g.rowVerbs('pin', true).addressable,
    '*Copy Path* must NOT be live on a pin: it would copy the literal sentinel',
  )
  ok(
    !g.rowVerbs('pin', true).expandable,
    'no twisty — Rust refuses to mark a pin expanded, so one would fold nothing',
  )

  /*
   * And the disk-changing verbs are refused *as a heading*, by shape rather than by a prefix
   * list — which is why nothing in `rowPaths.ts` had to learn about this row at all.
   */
  const WRITABLE = ['/home/u/work/cide', '/home/u/.local/state/cide/scratches/4f2a9c7b']
  ok(
    rp.mutationRefusal([PIN], WRITABLE)?.includes('heading'),
    'Rename, Cut, Paste and Move to Trash are refused on the pin, in the words the menu greys ' +
      'it with',
  )
  ok(
    rp.mutationRefusal([`${WRITABLE[0]}/src/main.rs`, PIN], WRITABLE) !== null,
    'one pin anywhere in a multi-row selection refuses the whole set',
  )

  // --- the icon ----------------------------------------------------------------------------

  eq(
    g.groupIcon('pin', false, g.PROJECT_NOTES),
    'markdown',
    'the row draws the glyph of the file it opens, so it looks like the tab it produces',
  )
  eq(
    g.groupIcon('pin', true, g.PROJECT_NOTES),
    'markdown',
    'a pin has no open variant — it never folds, so `markdown-open` would name nothing',
  )
  eq(
    g.groupIcon('pin', false, 'never-heard-of-it'),
    'document',
    'an unknown pin draws a document rather than a folder: a pin stands for one file',
  )
  /*
   * On disk, because a stem with no file is a 404 and a blank row rather than an error — the
   * same check `check:scratch` makes for its two folder stems.
   */
  ok(existsSync('public/icons/markdown.svg'), 'the markdown glyph ships in public/icons')
  ok(existsSync('public/icons/markdown_light.svg'), 'and so does its light variant')

  // --- the two spellings of `TreeRowKind` --------------------------------------------------

  /*
   * `groupRows.ts` is import-free so `check:groups` can compile it standalone, which means its
   * `RowKind` union is a hand-maintained copy of the generated one. This is the only thing that
   * notices when they come apart — and they come apart *silently*: an unknown kind falls into
   * `rowVerbs`'s `default` arm and draws an inert row.
   */
  const wire = /export type TreeRowKind = ([^;]+);/.exec(generated)?.[1] ?? ''
  const mirror = /export type RowKind = ([^\n]+)\n/.exec(rowsSrc)?.[1] ?? ''
  const members = (text) => [...text.matchAll(/'([a-z]+)'|"([a-z]+)"/g)]
    .map((m) => m[1] ?? m[2])
    .sort()
  ok(members(wire).length > 0, 'the generated `TreeRowKind` was found')
  ok(members(wire).includes('pin'), '`TreeRowKind` on the wire has the `pin` variant')
  eq(
    members(mirror),
    members(wire),
    "`groupRows.RowKind` names exactly the wire's kinds — the union is hand-maintained, so this " +
      'assertion is the whole of what keeps it honest',
  )

  // --- the strings that cross the wire -----------------------------------------------------

  ok(
    /pub const GROUP_ID: &str = "projectNotes";/.test(notesRs),
    '`cide_app::notes::GROUP_ID` is the id `groupRows.PROJECT_NOTES` mirrors; a rename on ' +
      'either side orphans the row',
  )
  ok(
    /pub const GROUP_LABEL: &str = "Project Notes";/.test(notesRs),
    'and the label is the one the row draws',
  )
  ok(
    /'file\.projectNotes'/.test(dispatch) && /Command::new\("file\.projectNotes"/.test(commandsRs),
    'the command id is spelled the same in the registry and in the dispatcher — `check:commands` ' +
      'enforces the pairing from the other side',
  )

  // --- Rust: where the file lives, and the line that must not truncate ----------------------

  ok(
    /state_dir\(\)\.join\("notes"\)/.test(coreNotes),
    'the notes file lives under `$XDG_STATE_HOME/cide/notes`, not inside the project — a file ' +
      'in the working tree would be drawn twice, or gitignored and drawn never',
  )
  ok(
    /crate::scratch::key\(primary_root\)/.test(stripComments(coreNotes)),
    'and it is keyed by `scratch::key` rather than by a third copy of the hashing rule',
  )
  ok(
    /notes\.md/.test(coreNotes),
    'the basename ends in .md, or the tab gets no markdown grammar and reads `Plain Text`',
  )
  ok(
    /create_new\(true\)/.test(stripComments(coreNotes)),
    'THE line that can destroy data: `create_new`, never `File::create` — an `ensure` that ' +
      'truncated would erase the notes on the second click, silently and with no undo',
  )
  ok(
    !/File::create|truncate\(true\)/.test(stripComments(coreNotes)),
    'and nothing in the module truncates by another spelling',
  )

  // --- Rust: the pin is drawn, and drawn first ---------------------------------------------

  ok(
    /pub fn pin\(&mut self, id: &str, label: &str\)/.test(fsGroups),
    '`cide_fs::groups::Groups::pin` is the mechanism — a pin is not a group with one child',
  )
  ok(
    /TreeRowKind::Pin/.test(fsGroups) && /has_children: !group\.pin/.test(stripComments(fsGroups)),
    'a pin draws as `Pin` with no twisty',
  )
  const prepare = /pub fn prepare\(&self[\s\S]*?\n    \}/.exec(stripComments(appGroups))?.[0] ?? ''
  ok(prepare.length > 0, "`ProjectGroups::prepare` was found")
  ok(
    prepare.indexOf('show_notes') >= 0 &&
      prepare.indexOf('show_notes') < prepare.indexOf('probe_libraries'),
    'and `prepare` shows the pin BEFORE External Libraries — the row order the report asks for',
  )
  ok(
    !/notes/.test(
      /pub fn writable_dirs\(&self\) -> Vec<PathBuf> \{[\s\S]*?\n    \}/.exec(appGroups)?.[0] ?? 'notes',
    ),
    '`writable_dirs` does NOT include the notes file: the row is pinned to one path, so a ' +
      'rename would orphan the pin and the next click would create a second empty notes.md',
  )

  // --- the wire surface --------------------------------------------------------------------

  ok(
    /pub async fn fs_notes_ensure/.test(cmdFs),
    '`fs_notes_ensure` is a command in `cmd::fs`',
  )
  /** The handler's body, comments gone and whitespace flattened, so a reformat cannot fail it. */
  const notesCommand = stripComments(cmdFs)
    .slice(stripComments(cmdFs).indexOf('pub async fn fs_notes_ensure'))
    .slice(0, 1200)
    .replace(/\s+/g, ' ')
  ok(/"fs_notes_ensure"/.test(contract), 'and `contract/commands.json` was accepted with it')
  ok(
    /notesEnsure: \(projectId: ProjectId\) =>/.test(client) &&
      /invoke<string>\('fs_notes_ensure'/.test(client),
    '`client.ts` — the frontend\'s only seam to `invoke` — exposes it',
  )
  ok(
    /\.roots \.first\(\)/.test(notesCommand),
    'and it keys by `roots[0]`: a multi-root project has ONE notes file, which is what "per ' +
      'project" means to the person using it',
  )

  // --- and that the whole thing is actually wired ------------------------------------------
  //
  // Everything above proves the rules are right. This half proves something calls them, which is
  // the half this project keeps getting wrong: features have shipped complete, correct and
  // reachable from nothing.

  const treeCode = stripComments(tree)
  ok(
    /row\.kind === 'pin' \? groupIdOf\(row\.path\) : null/.test(treeCode),
    "`FileTree`'s open path sends a pin's ID to the host and never its sentinel path — handing " +
      'the sentinel to `onOpen` would call `tab_open_file` with a string Rust refuses',
  )
  ok(
    /if \(row\.kind === 'pin'\) \{/.test(treeCode),
    'and a right-click on the pin gets its own one-item menu rather than fifteen greyed rows',
  )
  ok(
    /onOpenPin/.test(treeCode) && /onOpenPin/.test(stripComments(explorer)),
    '`FileTree` and `Explorer` both carry the `onOpenPin` seam',
  )
  ok(
    /row\.kind === 'pin'/.test(
      /const synthetic = [^\n]*\n/.exec(treeCode)?.[0] ?? '',
    ),
    'a pin counts as synthetic in the renderer, or the git-status ancestor walk runs per row ' +
      'per frame for an answer that is always clean',
  )
  const appCode = stripComments(app)
  ok(
    /onOpenPin=\{\(id\) =>/.test(appCode) && /runCommand\('file\.projectNotes'/.test(appCode),
    "`App.tsx` routes the pin's open through the COMMAND, so the double-click, the menu item " +
      'and the palette row cannot behave in three ways',
  )
  const dispatchCode = stripComments(dispatch)
  ok(
    /case 'file\.projectNotes':/.test(dispatchCode),
    '`dispatch.ts` handles the command — `check:commands` enforces this from the other side too',
  )
  ok(
    /fsApi\.notesEnsure\(/.test(dispatchCode),
    'the handler ensures the file exists BEFORE opening it: nothing else creates it',
  )
  ok(
    /fileApi\.open\(/.test(
      /case 'file\.projectNotes': \{[\s\S]*?\n      \}/.exec(dispatchCode)?.[0] ?? '',
    ),
    'and opens it as an ORDINARY file tab, which is why save, undo, find-in-file and the ' +
      'markdown mode need no special case anywhere',
  )
  ok(
    /reveal\(groupPath\(PROJECT_NOTES\)\)/.test(dispatchCode),
    'and reveals the pinned row by the id this module exports — `reveal`, not `revealGroup`, ' +
      'because a pin has nothing to expand',
  )
  ok(
    !/revealGroup\(PROJECT_NOTES\)/.test(dispatchCode),
    'never `revealGroup`: it issues an `fs_expand` a pin refuses to honour',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('project notes: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
