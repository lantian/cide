/**
 * Scratch files — every link in the chain from ⇧⌥S to a saved buffer.
 *
 * # What this is defending
 *
 * A scratch is a file with no project, so almost everything about it is a *rule* rather than a
 * mechanism, and every one of those rules lives somewhere a screenshot cannot see:
 *
 *   * the type list must be the list the editor's language loader knows about, or a scratch
 *     offered as *YAML* opens with no highlighting (pinned in `check:editor`, beside the table);
 *   * the file lives outside every project root, so every containment check in `cmd::fs` has to
 *     admit its directory and **nothing else** — a scratch you cannot save is not a scratch, and
 *     a widening that also admitted `/etc` would be worse than the feature is good;
 *   * the tree group nothing watches has to be re-listed by whatever changed it;
 *   * and the whole thing is reachable by exactly one chord and one palette row, which is where
 *     this project's fourteen previous dead features died.
 *
 * The pure half — the row rules for a writable group — is compiled and driven. The wiring half
 * is asserted in source, because those bugs are *missing calls* rather than wrong functions and
 * no module's own tests can see one. The Rust half (the drawer, the keying, the claim-is-the-
 * creation loop, the re-listing after every mutation) is covered by
 * `cide_core::scratch`'s tests and `cide_app::cmd::fs::scratch_tests`, which drive the real
 * command layer; this file pins that the two sides agree on the strings that cross between them.
 *
 * Run: `pnpm --dir ui run check:scratch`
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-scratch-'))
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
 * assertion written that way stays green after the code is deleted and only the prose is left.
 * That is a lesson this repository has paid for; `check-paths.mjs` and `windows.rs` strip for
 * the same reason. String-aware, so a `'//'` inside a literal does not eat the rest of the line.
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

  // The three source files that have to agree on strings nobody type-checks.
  const scratchesRs = read('../../crates/cide-app/src/scratches.rs')
  const coreScratch = read('../../crates/cide-core/src/scratch.rs')
  const commandsRs = read('../../crates/cide-core/src/commands.rs')
  const keymapRs = read('../../crates/cide-core/src/keymap.rs')
  const dispatch = read('../src/keys/dispatch.ts')
  const client = read('../src/ipc/client.ts')
  const app = read('../src/App.tsx')
  const store = read('../src/sidebar/treeStore.ts')
  const tree = read('../src/sidebar/FileTree.tsx')
  const host = read('../src/overlays/OverlayHost.tsx')
  const overlayStore = read('../src/overlays/store.ts')
  const picker = stripComments(read('../src/overlays/ScratchType.tsx'))

  // --- 1. the id, which is a string two languages share ------------------------------------

  eq(g.SCRATCHES, 'scratches', 'the group id the frontend knows')
  ok(
    /pub const GROUP_ID: &str = "scratches";/.test(scratchesRs),
    '…and `cide_app::scratches::GROUP_ID` is the same string. A rename on one side silently ' +
      'orphans `view.scratches`: the reveal would ask for a group that does not exist and the ' +
      'command would report "this project has no Scratches group" for ever',
  )
  eq(
    g.groupPath(g.SCRATCHES),
    'cide://group/scratches',
    'the header path is the Rust sentinel verbatim',
  )
  ok(
    !g.groupPath(g.SCRATCHES).startsWith('/'),
    'and it is not absolute, so `check_within` refuses it exactly as it refuses the other ' +
      "group's header — a heading is not a file however writable the group is",
  )

  // --- 2. the row rules for a group cide *may* write in ------------------------------------
  //
  // This is the one thing about Scratches that is genuinely new to the row rules, and it needed
  // no new rule at all: `rowVerbs` already takes "may cide change things here" as a parameter
  // rather than deriving it. What changed is where the caller gets the answer — the writable
  // set from `fs_writable_roots`, not the project's roots.

  const verbs = (kind, inProject) => {
    const v = g.rowVerbs(kind, inProject)
    return [v.expandable, v.openable, v.actionable, v.mutable, v.addressable]
  }
  eq(
    verbs('file', true),
    [false, true, true, true, true],
    'a scratch opens, is copyable, and IS renamed, cut and trashed — that is the difference ' +
      'from a dependency source, which is the same kind of row with the flag off',
  )
  eq(
    verbs('group', true),
    [true, false, false, false, false],
    'and the header still only folds, writable group or not: it has no path on disk',
  )

  const ROOTS = ['/home/u/work/cide']
  const DRAWER = '/home/u/.local/state/cide/scratches/4f2a9c7b1d0e'
  const SCRATCH = `${DRAWER}/scratch.rs`
  const WRITABLE = [...ROOTS, DRAWER]

  eq(
    rp.mutationRefusal([SCRATCH], ROOTS),
    'This file is outside the project, so cide will not move, rename or delete it',
    'with only the roots, a scratch is refused — which is what the panel does for the frame ' +
      'before `fs_writable_roots` answers, and is the safe direction of being briefly wrong',
  )
  eq(
    rp.mutationRefusal([SCRATCH], WRITABLE),
    null,
    'and with the drawer in the list it is renamed, cut and trashed like any other file. This ' +
      'is the assertion that makes a scratch a file rather than a decoration',
  )
  eq(
    rp.mutationRefusal([`${DRAWER}.json`], WRITABLE),
    'This file is outside the project, so cide will not move, rename or delete it',
    'the origin record is a *sibling* of the drawer, not a file in it, and the containment ' +
      'test is component-wise — a string prefix would make `<key>.json` renamable',
  )
  eq(
    rp.mutationRefusal(['/etc/passwd'], WRITABLE),
    'This file is outside the project, so cide will not move, rename or delete it',
    'widening containment for the drawer must not widen it for anything else',
  )
  eq(
    rp.mutationRefusal([`${ROOTS[0]}/src/main.rs`, SCRATCH], WRITABLE),
    null,
    'a mixed selection of a project file and a scratch is entirely inside the writable set',
  )

  /*
   * The narrower refusal. A scratch is renamed and deleted; the *directory* holding it is not a
   * folder anybody fills, because its name is a blake3 and its contents are supposed to be
   * scratches.
   */
  eq(rp.creationRefusal(`${ROOTS[0]}/src/main.rs`, ROOTS), null, 'a project file has a parent to fill')
  eq(rp.creationRefusal(ROOTS[0], ROOTS), null, 'and so does a project root')
  ok(
    rp.creationRefusal(SCRATCH, ROOTS)?.includes('New scratch file'),
    'the drawer refuses *New File in 4f2a9c7b…* and names the command that does work there. ' +
      'Rust would happily create the file; this is a policy, and the label would be a hash',
  )
  ok(
    rp.creationRefusal(DRAWER, ROOTS) !== null,
    'the drawer itself too, not only the files in it',
  )

  /*
   * The header's glyph, and that the file behind it exists.
   *
   * `groupIcon` names a Material Icon Theme stem, and a stem with no file in `public/icons/` is
   * a 404 and a blank row rather than an error — nothing in the running app would say a word.
   * The two groups take different folders deliberately: one library icon on both headers would
   * make them read as two halves of one thing.
   */
  eq(g.groupIcon('group', false, g.SCRATCHES), 'folder-temp', 'the drawer draws a temp folder')
  for (const stem of ['folder-temp', 'folder-temp-open']) {
    ok(
      existsSync(new URL(`../public/icons/${stem}.svg`, import.meta.url)),
      `public/icons/${stem}.svg ships — a stem with no file is a blank row and no error`,
    )
  }

  // --- 3. the drawer is Rust's answer, asked once, and used by the menu --------------------

  ok(
    /fs_writable_roots/.test(client) && /writableRoots:/.test(client),
    '`ipc/client.ts` has the only seam to `fs_writable_roots`',
  )
  ok(
    /fsApi\.writableRoots\(project\)/.test(store),
    '`treeStore.attach` asks for it — the drawer is a blake3 under $XDG_STATE_HOME and cannot ' +
      'be derived in the webview',
  )
  ok(
    /writable: readonly string\[\]/.test(store),
    '…and keeps it, so the menu does not re-ask per right-click',
  )
  ok(
    /const writable = useMemo\(\(\) => \[\.\.\.roots, \.\.\.extraWritable\], \[roots, extraWritable\]\)/.test(
      tree,
    ),
    '`FileTree` unions it with the workspace mirror\'s roots rather than replacing them, so a ' +
      'project file is mutable from the first frame instead of from the first round trip',
  )
  ok(
    /rowVerbs\(kind, rootOf\(path, writable\) !== null\)/.test(tree),
    'and the context menu asks `rowVerbs` with the **writable** set. With `roots` there, every ' +
      'scratch row would be greyed with "outside the project" and the feature would be ' +
      'read-only in the one panel that shows it',
  )
  ok(
    /creationRefusal\(row\.path, roots\)/.test(tree),
    'while *New File…* / *Paste* ask with the **roots**, which is the whole point of there ' +
      'being two rules: rename yes, create-inside no',
  )

  // --- 4. reachability: the chord, the palette, the picker, the tab ------------------------

  ok(
    /Command::new\("scratch\.new", "New scratch file…", FILE\)/.test(commandsRs),
    '`scratch.new` is in the registry, with the ellipsis every command that asks a question first carries',
  )
  ok(
    /\("alt\+shift\+s", "scratch\.new"\)/.test(keymapRs),
    'and ⇧⌥S is bound to it. A feature reachable only from the palette is how this project ' +
      'has shipped fourteen dead controls',
  )
  ok(
    /Command::new\("view\.scratches", "Show Scratches", VIEW\)/.test(commandsRs),
    '`view.scratches` exists too: the group is the last row of a virtualized tree, so without ' +
      'a command it is drawn and, on any repository with depth, unreachable',
  )
  ok(
    /case 'scratch\.new': \{/.test(dispatch) && /case 'view\.scratches': \{/.test(dispatch),
    'both are dispatched — `check:commands` enforces this from the other side too',
  )
  ok(
    /revealGroup\(SCRATCHES\)/.test(dispatch),
    '`view.scratches` reveals the group by the id this module exports, not by a second spelling',
  )
  ok(
    /toggle\('scratch'\)/.test(dispatch),
    "`scratch.new` *toggles* the picker: the gate swallows ⇧⌥S either way, so `show` would " +
      'leave the chord unable to dismiss what the chord opened',
  )
  ok(
    /\| 'scratch'/.test(overlayStore),
    "'scratch' is an OverlayKind",
  )
  ok(
    /if \(open === 'scratch' && project === null\) close\(\)/.test(host),
    'and an overlay that cannot function is **closed**, not rendered null — a store left open ' +
      'makes `overlayOpen()` lie and the next ⇧⌥S toggles a phantom shut',
  )
  ok(
    /<ScratchType onDismiss=\{close\} onCreate=\{actions\.createScratch\} \/>/.test(host),
    'the host mounts the picker and hands it the action',
  )

  // --- 5. what accepting a type actually does ----------------------------------------------

  const create = (() => {
    const start = app.indexOf('createScratch: (ext) => {')
    return start < 0 ? '' : app.slice(start, app.indexOf('runCommand: (id) =>', start))
  })()
  ok(create.length > 0, '`App.tsx` supplies `createScratch`')
  ok(
    /fsApi\.scratchNew\(activeProjectId, ext\)/.test(create),
    '…which creates the file',
  )
  ok(
    /fileApi\.open\(activeProjectId, path\)/.test(create),
    '…opens it as a tab — a scratch nobody can type into is not a scratch',
  )
  ok(
    /useFileTree\.getState\(\)\.reveal\(path\)/.test(create),
    '…and selects its row, so the user can see where it went',
  )
  ok(
    /if \(!shown\)/.test(create) && /notify\(/.test(create),
    '…**and reads whether the reveal landed**. `treeStore.reveal` used to swallow that answer, ' +
      'which is exactly how `file.reveal` came to be a listed, bound command that opened a ' +
      'panel and then did nothing',
  )

  // --- 6. the picker itself ----------------------------------------------------------------

  ok(
    /from '@\/editor\/languages'/.test(picker) && /SCRATCH_TYPES/.test(picker),
    'the picker lists `SCRATCH_TYPES` from the language table rather than an array of its own. ' +
      'A second list is how a type gets offered that the editor cannot highlight, and ' +
      '`check:editor` pins the table against `lookup()` on the other side',
  )
  ok(
    /defaultScratchType\(focusedTabPath\(boot\)\)/.test(picker),
    'and opens on the type of the file the user is looking at, which makes the common case ⇧⌥S ⏎',
  )
  ok(
    /listAction\(ev, shown\.length, at\)/.test(picker),
    'arrows, Enter and Escape come from `overlays/listKeys.ts`, so this list cannot feel ' +
      'different from the other four — and it is asked about the FILTERED list, because `at` ' +
      'indexes that one. Handing it `SCRATCH_TYPES.length` would let Down walk past the end of ' +
      'what is on screen onto rows the query excluded',
  )
  ok(
    /type\.ext/.test(picker),
    'and the row shows the extension, because that is what the file will be called and what ' +
      'decides the highlighting',
  )

  // --- 6b. the filter field, and where the caret is when the popup opens --------------------
  //
  // The focus handoff is the assertion this section exists for. An overlay opened by a keystroke
  // that is not focused on its first frame sends the user's next character to whatever had focus
  // before — a terminal, which receives it as shell input. That bug has shipped here once
  // already, out of the find bar, and "it is focused, usually" reads perfectly well in a review.

  ok(
    /useLayoutEffect\(\(\) => \{\s*field\.current\?\.focus\(\)/.test(picker),
    'the filter field is focused in a **layout** effect. `ModalShell` states the reason at ' +
      'length: a passive `useEffect` leaves one frame in which the field is not focused, and ' +
      'the character typed in that frame goes to the terminal underneath. `GoToLine` uses the ' +
      'weaker spelling; this must not',
  )
  ok(
    /<input[\s\S]{0,600}?onKeyDown=/.test(picker),
    'and the keys are handled on the field rather than on the box. The box used to hold focus ' +
      'because there was no field; with one, a handler left on the box would answer for a ' +
      'caret that is somewhere else',
  )
  ok(
    !/tabIndex=\{-1\}/.test(picker),
    'and the box no longer takes the caret itself — two focusable things in a fourteen-row ' +
      'popup is how Enter ends up acting on a row the highlight is not on',
  )
  ok(/data-audit="scratchFilter"/.test(picker), 'the field is auditable by name')
  ok(
    /filterScratchTypes\(query\)/.test(picker) && /from '@\/editor\/languages'/.test(picker),
    'the filtering rule comes from `editor/languages.ts` beside the list it filters, not from a ' +
      'predicate inside this component — which is also what makes it drivable by `check:editor`',
  )
  ok(
    /matchCounter\(shown\.length, SCRATCH_TYPES\.length\)/.test(picker),
    'the head counts what is SHOWN against the total. It was the constant `13 types`, which a ' +
      'filter turns into a number that contradicts the list under it',
  )
  ok(
    /No type matches/.test(picker) && /shown\.length === 0/.test(picker),
    'and a query that matches nothing says so where the list would be. `listAction` already ' +
      'answers `none` for Enter with an empty list, so the popup correctly stays open and does ' +
      'nothing — which without a sentence is the fifth indistinguishable empty state',
  )
  ok(
    !/onCreate\(query/.test(picker),
    'and Enter on an empty list does NOT create a scratch of the typed extension: that is a ' +
      'free-text path into `cide_core::scratch::check_ext` and a separate decision',
  )

  // --- 7. the Rust side's own promises, in the strings that cross ---------------------------

  ok(
    /scratches\/<key>/.test(coreScratch) || /scratches\/<blake3/.test(coreScratch),
    'the drawer is under `$XDG_STATE_HOME/cide/scratches/<key>` and the module says so',
  )
  ok(
    /fn scratch_name\(ext: &str, attempt: u32\)/.test(coreScratch) &&
      /format!\("scratch_\{attempt\}\.\{ext\}"\)/.test(coreScratch),
    "the naming is IDEA's `scratch_1.rs` and not this codebase's ` copy` house rule — a scratch " +
      'is not a copy of anything',
  )
  ok(
    /create_new\(true\)/.test(coreScratch),
    'and the claim IS the creation: `create_new` fails with EEXIST in the kernel, so two ' +
      'windows asking in the same millisecond cannot land on one file',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('scratch files: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
