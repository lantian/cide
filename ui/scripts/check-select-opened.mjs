/**
 * *Select opened file* — the whole chain from a keystroke to a highlighted row.
 *
 * # Why a script of its own
 *
 * Because nothing was wrong with this feature. `Index::reveal`, `fs_reveal`, `fs.reveal`,
 * `treeStore.reveal`, the scroll effect, the command registry entry and the `dispatch.ts` arm
 * all existed and all worked, and the only way to run it was to type "Reveal file in sidebar"
 * into the command palette **while an editor pane had focus**. There was no binding, no button,
 * and the `when` clause was strictly stronger than the handler's real precondition — so the one
 * route that existed was also filtered out of the palette in a file tab split with a shell pane.
 *
 * That is this project's recurring defect one stage further along than usual: not "implemented
 * and nothing calls it", but "implemented, called, and reachable from almost nothing". A test of
 * the reveal *logic* cannot see it. So this file asserts the wiring, in source, end to end:
 *
 *   1. the chord is in `cide_core::keymap::defaults()` and is unconditional there;
 *   2. the palette clause is the handler's actual precondition, and every flag it names is in
 *      `CONTEXT_FLAGS` **and** derived by `keys/context.ts`;
 *   3. `dispatch.ts` reports every refusal with a *sentence* rather than with `unmet`, which
 *      writes to a diagnostic log nobody reads — with a hotkey on the command, that is a key
 *      that does nothing;
 *   4. it asks for the keyboard afterwards, through `chrome/focusRequests.ts`, and `FileTree`
 *      consumes the request;
 *   5. the Explorer's button routes through `runCommand`, not through `treeStore.reveal`, so
 *      the button, the chord and the palette row cannot drift into three behaviours;
 *   6. and it is disabled *with a reason* rather than hidden when no file tab is open.
 *
 * The pure half — `focusedTabPath` — is compiled and driven, because "which file is the opened
 * file" is a real rule with a real edge (a git diff's `newPath` is repo-relative).
 *
 * Run: `pnpm --dir ui run check:select-opened`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-select-opened-'))
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

try {
  // --- the pure half: which file is "the opened file" ------------------------------------
  //
  // `keys/target.ts` imports only types, so `tsc` compiles it standalone the same way
  // `check-key-gate.mjs` compiles the four key modules.
  //
  // Through a generated `tsconfig.json` rather than bare flags, because this module names its
  // types through the `@/` alias and `paths` is the one option `tsc` refuses on the command
  // line. The config is written into the output directory and points back at the real `src`.
  const config = join(out, 'tsconfig.json')
  writeFileSync(
    config,
    JSON.stringify({
      compilerOptions: {
        outDir: out,
        rootDir: resolve('src'),
        module: 'esnext',
        target: 'es2022',
        moduleResolution: 'bundler',
        strict: true,
        noUncheckedIndexedAccess: true,
        exactOptionalPropertyTypes: true,
        skipLibCheck: true,
        baseUrl: resolve('.'),
        paths: { '@/*': ['src/*'] },
      },
      files: [resolve('src/keys/target.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', config], {
    stdio: 'inherit',
  })
  const target = await import(`file://${join(out, 'keys', 'target.js')}`)

  /** A shell-window bootstrap showing one project whose active tab is `kind`. */
  const boot = (kind) => ({
    role: { kind: 'shell', active: 'p', projects: ['p'] },
    workspace: {
      projects: {
        p: {
          id: 'p',
          activeTab: 't',
          tabs: [{ id: 't', kind, tree: { focused: 'x', panes: {} } }],
          roots: [{ path: '/p' }],
        },
      },
    },
  })

  eq(target.focusedTabPath(null), null, 'no bootstrap, no file')
  eq(
    target.focusedTabPath(boot({ kind: 'claudeHome' })),
    null,
    'the pinned Claude console is not a file, so the command has nothing to select',
  )
  eq(
    target.focusedTabPath(boot({ kind: 'file', path: '/p/src/main.rs', dirty: false })),
    '/p/src/main.rs',
    'a file tab names its file',
  )
  /*
   * A **diff** tab names the file it is diffing, which is what IDEA reveals — but only when
   * that name is absolute. `DiffSpec.newPath` is repo-relative for a `Git` diff (its own doc
   * comment says so), and handing a relative string to `fs_reveal` would find nothing and make
   * the command report that a file "is not in this project's file tree" while naming something
   * that is not a path.
   */
  eq(
    target.focusedTabPath(
      boot({ kind: 'diff', spec: { oldPath: '/p/a.rs', newPath: '/p/b.rs' } }),
    ),
    '/p/b.rs',
    'a diff tab names the new side, so Select opened file works over a diff',
  )
  eq(
    target.focusedTabPath(boot({ kind: 'diff', spec: { oldPath: 'src/a.rs', newPath: 'src/b.rs' } })),
    null,
    "a git diff's newPath is repo-relative, and a relative path is not something to reveal",
  )
  /* A detached-pane window shows no tab at all — and no sidebar, so there is nowhere to reveal. */
  eq(
    target.focusedTabPath({
      role: { kind: 'detachedPane', project: 'p', tab: 't', pane: 'x' },
      workspace: { projects: { p: { id: 'p', activeTab: 't', tabs: [], roots: [] } } },
    }),
    null,
    'a detached-pane window names no file, however busy the shell window is',
  )

  /*
   * And it must agree with `chrome/menuModel.ts::pathOf`, which answers the same question for
   * the tab strip's menu. The two cannot import each other — that module is compiled standalone
   * and its header forbids a value import — so the agreement is asserted here instead.
   */
  const menuModel = read('../src/chrome/menuModel.ts')
  ok(
    /if \(tab\.kind\.kind === 'file'\) return tab\.kind\.path/.test(menuModel) &&
      /if \(tab\.kind\.kind === 'diff'\) return tab\.kind\.spec\.newPath/.test(menuModel),
    '`menuModel.pathOf` still answers file-then-diff, which is what `focusedTabPath` mirrors',
  )

  // --- 1. the chord ------------------------------------------------------------------------

  const keymapRs = read('../../crates/cide-core/src/keymap.rs')
  ok(
    /\("ctrl\+shift\+e", "file\.reveal"\)/.test(keymapRs),
    'ctrl+shift+e is bound to file.reveal in cide_core::keymap::defaults() — without this the ' +
      'command is reachable only by typing its name into the palette',
  )
  /*
   * Unconditional, and that is a decision rather than an omission. A `Binding::when` gates the
   * *keyboard*, so an `editorFocused` clause would make the chord dead in a terminal and in the
   * file tree — which is where somebody asking "where is the file I am editing?" most often has
   * their hands. The `.chain(...)` block is where the clause-carrying bindings live; this
   * asserts the binding is not in it.
   */
  const chained = keymapRs.slice(keymapRs.indexOf('.chain('))
  ok(
    !/ctrl\+shift\+e/.test(chained),
    'and it carries no `when`: a clause here would make the chord dead in a terminal pane, ' +
      'which is where the question is usually asked from',
  )

  // --- 2. the palette clause, and the flags it names --------------------------------------

  const commandsRs = read('../../crates/cide-core/src/commands.rs')
  const entry = /Command::new\("file\.reveal", "([^"]+)", \w+\)\s*\.when\("([^"]+)"\)/.exec(
    commandsRs,
  )
  ok(entry !== null, '`file.reveal` is registered with a title and a clause')
  if (entry) {
    eq(entry[1], 'Select opened file', "the title is IDEA's, which is what a user types")
    eq(
      entry[2],
      'shellWindow && fileTabActive',
      'the clause is the handler\'s actual precondition. `editorFocused` was strictly stronger ' +
        'and hid a working command: it is about the focused *pane*, so a file tab split with a ' +
        'shell pane filtered the row out of the only list that offers it',
    )
  }
  ok(
    /"fileTabActive",/.test(commandsRs.slice(commandsRs.indexOf('pub const CONTEXT_FLAGS'))),
    '`fileTabActive` is in CONTEXT_FLAGS — an unlisted flag reads false for ever and the ' +
      'command silently vanishes from the palette, which is what happened to `repoOpen`',
  )
  const context = read('../src/keys/context.ts')
  ok(
    /'fileTabActive',/.test(context) && /fileTabActive: focusedTabPath\(boot\) !== null/.test(context),
    '…and `keys/context.ts` both lists and *derives* it, from the same function the handler ' +
      'and the Explorer button use — a flag named by a supplier that is a constant is the ' +
      'second, worse version of that bug and it also shipped',
  )

  // --- 3-4. the handler: a sentence per refusal, and the keyboard --------------------------

  const dispatch = read('../src/keys/dispatch.ts')
  const arm = (() => {
    const start = dispatch.indexOf("case 'file.reveal': {")
    const end = dispatch.indexOf("case 'view.externalLibraries'", start)
    return dispatch.slice(start, end < 0 ? start + 4000 : end)
  })()
  ok(arm.length > 0, "dispatch.ts still has a `case 'file.reveal'`")
  ok(
    !/unmet\(/.test(arm),
    'and not one refusal in it is `unmet`. `unmet` writes a line to the diagnostic log and ' +
      'nothing to the screen; with a hotkey on the command that is a key that does nothing, ' +
      'which is the defect this project has now found fifteen times',
  )
  ok(
    (arm.match(/notify\(/g) ?? []).length >= 3,
    'every branch that cannot act says so: no sidebar, no file tab, and no row for the file',
  )
  ok(
    /focusedTabPath\(boot\(\)\)/.test(arm),
    'the path comes from `focusedTabPath`, so the palette row and the button work with no ' +
      'argument — and a diff tab reveals the file it is diffing',
  )
  ok(
    /requestFocus\('fileTree'\)/.test(arm),
    'and the tree is given the keyboard once the reveal lands. Without it the row lights up ' +
      'and the arrows still go to the terminal the chord was pressed in',
  )
  ok(
    /\.then\(\(shown\) =>/.test(arm),
    "the reveal's answer is read rather than voided — `fs_reveal` says `null` for a gitignored " +
      'file and for one deleted since it was opened',
  )

  const focusRequests = read('../src/chrome/focusRequests.ts')
  ok(/\| 'fileTree'/.test(focusRequests), "'fileTree' is a FocusSurface")
  const tree = read('../src/sidebar/FileTree.tsx')
  ok(
    /useFocusRequested\('fileTree'\)/.test(tree) && /clearFocusRequest\('fileTree'\)/.test(tree),
    '`FileTree` consumes **and clears** the request. A request that is observed but never ' +
      'cleared fires again on the next mount, so merely looking at the Files panel would ' +
      'snatch the caret out of a terminal',
  )
  ok(
    /scrollRef\.current\?\.focus\(\)/.test(tree),
    'and what it focuses is the scroller, which is the tree\'s single tab stop',
  )

  // --- 5-6. the button ---------------------------------------------------------------------

  const explorer = read('../src/sidebar/Explorer.tsx')
  ok(
    /onSelectOpened/.test(explorer) && /styles\.headerAction/.test(explorer),
    'the Explorer header draws the button',
  )
  ok(
    /disabled=\{openedFile === null\}/.test(explorer),
    'disabled when no file tab is open, rather than hidden: a control that renders nothing ' +
      'does not read as "not applicable", it reads as broken — which is a user report this ' +
      'project has already had',
  )
  ok(
    /No file is open in this tab/.test(explorer),
    '…and it says why, in both `title` and `aria-label`. A greyed control with no reason is ' +
      'the thing people file bugs about',
  )
  const css = read('../src/sidebar/FileTree.module.css')
  ok(/\.headerAction \{/.test(css), 'and the class it names exists in the stylesheet')

  const app = read('../src/App.tsx')
  ok(
    /onSelectOpened=\{\(\) => runCommand\('file\.reveal', null\)\}/.test(app),
    'the click runs the **command**, not `treeStore.reveal`. A second call site with its own ' +
      'copy of the preconditions is how a button, a chord and a palette row come to behave in ' +
      'three different ways',
  )
  ok(
    /openedFile=\{focusedTabPath\(boot\)\}/.test(app),
    'and the enablement comes from the same function the clause and the handler use',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('select opened file: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
