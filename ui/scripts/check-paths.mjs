/**
 * Checks `src/terminal/pathMatch.ts` and `src/terminal/clickGate.ts` — the rules deciding what
 * in a pane's output is a file path, which file on disk it names, and what a mouse press over it
 * means.
 *
 * Same shape as `check-problems.mjs` and `check-exit-marker.mjs`, and for the same reason:
 * there is no JS test runner in this project, and both modules are pure and import-free so the
 * TypeScript in `node_modules` can compile them on their own and node can drive them.
 *
 * # The three failures this exists to prevent
 *
 * 1. **Lighting up prose.** A matcher that offers every dotted token is worse than no feature
 *    at all: it teaches the user that underlines mean nothing. So every negative below is as
 *    load-bearing as every positive, and they are drawn from what this repo's own toolchain
 *    actually prints — `0.12.0`, `v0.1.0`, `target(s)`, a truncated TUI path, a URL.
 * 2. **Opening the wrong file.** A candidate that resolves under two different bases must come
 *    back `many` and must never come back with the first one. That rule has no visible symptom
 *    when it breaks — the link still works, it just sometimes opens somebody else's file — so
 *    it is pinned from both directions here.
 * 3. **Handing the gesture to the child.** Added in M15, and it is the one this file was blind
 *    to. See below.
 *
 * The positives are transcriptions of real output. Where a shape looked surprising it was run
 * rather than recalled: rustc's `-->`, the trailing colon a Rust panic and esbuild both add,
 * tsc's two entirely different formats, ripgrep's `path:line:` where the third field is
 * content and not a column.
 *
 * # What this file could not see, and now can
 *
 * The pin at the foot of this file has always read:
 *
 * > a path parsed out of terminal bytes reaches a workspace tab and nothing else — not the
 * > desktop opener, not the file manager, not a new window
 *
 * That assertion was **true of cide's source and false of the user's screen**. A ctrl+click on
 * `Update(/home/…/ProjectSwitcher.tsx)` in a Claude pane opened the desktop file manager on
 * `/home/…/chrome`, because the click gate returned early when nothing had resolved yet, xterm's
 * own handler then wrote a mouse report to the pty, and `claude` answered it by forking
 * `dbus-send … org.freedesktop.FileManager1.ShowItems`.
 *
 * **A grep over cide cannot see a file manager opened by a program cide forwarded the click to.**
 * That is the class this file was structurally unable to catch, and it is worth naming rather
 * than quietly fixing: *anything cide does not swallow is a gesture cide has delegated*, so
 * "cide's source contains no opener call" is not evidence that no opener ran.
 *
 * What replaces it is not a better grep. `clickGate.ts` was created so that the press decision is
 * a **pure function that is not given the hover state at all** — it cannot consult it, so the
 * early return cannot come back — and this file drives that function directly. The two source
 * pins that remain are about ordering inside the DOM callback, and they run over
 * comment-stripped source so that the prose explaining the fix cannot satisfy them.
 *
 * Run: `pnpm --dir ui run check:paths`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-paths-'))

let failed = 0
const fail = (what, detail) => {
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
  failed++
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  /*
   * A bare `tsc` with no tsconfig, deliberately. Neither module imports anything — not even a
   * type through the `@/*` alias — and if this compile ever needs a tsconfig then something has
   * added an import and the node-testability of the rules has been lost, which is the whole
   * reason they live in their own modules.
   *
   * Two entry files rather than one, and they must stay independent of each other: `tsc` would
   * happily follow an import from one to the other and this compile would keep passing, but the
   * point of the arrangement is that each is drivable **alone**, so a future reader who deletes
   * one still has a compiling gate for the other.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/pathMatch.ts',
      'src/terminal/clickGate.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  const {
    matchPaths,
    candidatePaths,
    outsidePaths,
    resolveCandidate,
    resolveDirectory,
    MAX_LINE,
  } = await import(`file://${join(out, 'pathMatch.js')}`)
  const { pressVerdict, cellFromPoint, linkAtCell } = await import(
    `file://${join(out, 'clickGate.js')}`
  )

  const read = (rel) => readFileSync(join(UI, rel), 'utf8')
  /** Prose out. A pin that a COMMENT can satisfy is a pin the feature's deletion leaves green. */
  const code = (rel) =>
    read(rel)
      .replace(/\/\*[\s\S]*?\*\//g, '')
      .replace(/\/\/[^\n]*/g, '')

  /** The candidate texts a line yields, in order. */
  const texts = (line) => matchPaths(line).map((c) => c.text)
  /** The first candidate, or `null` — most samples are meant to yield exactly one. */
  const only = (line) => {
    const all = matchPaths(line)
    return all.length === 1 ? all[0] : { candidates: all.length, texts: all.map((c) => c.text) }
  }

  // --- what this repo's toolchain prints ------------------------------------------------

  eq(
    only('   --> crates/cide-app/src/lib.rs:270:13'),
    { text: 'crates/cide-app/src/lib.rs', start: 7, end: 40, line: 270, column: 13 },
    'rustc points with `-->`, and the arrow is not part of the path',
  )

  eq(
    only("thread 'main' (403710) panicked at src/main.rs:1:13:"),
    { text: 'src/main.rs', start: 35, end: 52, line: 1, column: 13 },
    'a Rust panic adds a trailing colon after the column, and it belongs to the link',
  )

  eq(
    only('src/bad.ts:1:7 - error TS2322: Type is not assignable'),
    { text: 'src/bad.ts', start: 0, end: 14, line: 1, column: 7 },
    'tsc --pretty (a PTY is a TTY, so this is the one a pane really shows)',
  )

  eq(
    only('src/bad.ts(1,7): error TS2322: Type is not assignable'),
    { text: 'src/bad.ts', start: 0, end: 15, line: 1, column: 7 },
    'tsc --pretty false puts the position in parentheses instead',
  )

  eq(
    only('  src/foo.tsx:12:3:'),
    { text: 'src/foo.tsx', start: 2, end: 19, line: 12, column: 3 },
    'esbuild/vite, trailing colon and all',
  )

  eq(
    only('    at f (/home/u/work/cide/ui/scripts/check-theme.mjs:42:11)'),
    {
      text: '/home/u/work/cide/ui/scripts/check-theme.mjs',
      start: 10,
      end: 60,
      line: 42,
      column: 11,
    },
    'a node stack frame — absolute, inside parentheses, and the paren is not part of it',
  )

  eq(texts(' M ui/package.json'), ['ui/package.json'], '`git status --short` is a two-column row')

  eq(
    texts('--- a/ui/package.json'),
    ['ui/package.json'],
    "git's pre-image prefix is not a directory anybody has",
  )
  eq(
    texts('+++ b/run.sh'),
    ['run.sh'],
    'and the diff header is itself the proof, so a top-level file survives having no slash left',
  )
  eq(
    texts('--- /dev/null'),
    ['/dev/null'],
    'a new file names /dev/null, which is a candidate here and dies at containment, not at the grammar',
  )

  eq(
    only('ui/src/App.tsx:830:                void fileApi.open(project, path)'),
    { text: 'ui/src/App.tsx', start: 0, end: 19, line: 830, column: null },
    'ripgrep prints `path:line:` and then content — the third field is not a column',
  )

  eq(texts('⏺ Read(ui/src/App.tsx)'), ['ui/src/App.tsx'], 'Claude Code names the files it reads')

  /*
   * The shapes a Claude pane actually prints, all four verbs plus the absolute form.
   *
   * These were added because the parenthesised form was *reported as the cause* of the
   * file-manager bug and was not — the matcher takes the file, never its parent — and an
   * assertion that was made to check a hypothesis is worth keeping whichever way it came out.
   * The absolute-in-parens case is the one nothing here covered before: every previous Claude
   * Code sample was relative.
   */
  eq(
    only('Update(/home/u/work/cide/ui/src/chrome/ProjectSwitcher.tsx)'),
    {
      text: '/home/u/work/cide/ui/src/chrome/ProjectSwitcher.tsx',
      start: 7,
      end: 58,
      line: null,
      column: null,
    },
    'THE REPORTED CASE: an absolute path inside Update(...) is the FILE, span-for-span. `(` and ' +
      '`)` are not body characters, so the parent directory this bug appeared to name could ' +
      'never have come from here — it came from the child cide forwarded the click to',
  )
  eq(
    only('⏺ Update(/home/u/work/cide/run.sh)'),
    { text: '/home/u/work/cide/run.sh', start: 9, end: 33, line: null, column: null },
    'and the ⏺ bullet the TUI prefixes shifts the span by two, not the text',
  )
  eq(
    texts('Write(crates/cide-app/src/lib.rs)'),
    ['crates/cide-app/src/lib.rs'],
    'Write(...) is the same shape as Update(...)',
  )
  eq(
    texts('Edit(ui/src/keys/gate.ts)'),
    ['ui/src/keys/gate.ts'],
    'and so is Edit(...)',
  )
  eq(
    only('Read(/home/u/p/f.tsx:42)'),
    { text: '/home/u/p/f.tsx', start: 5, end: 23, line: 42, column: null },
    'Read(path:line) still reads the position, and the span covers the `:42` inside the parens',
  )
  eq(
    texts('Bash(cat ui/src/App.tsx)'),
    ['ui/src/App.tsx'],
    'Bash(...) contributes whatever real path its command line holds, and not the command',
  )
  eq(
    texts('Bash(cargo test -p cide-git)'),
    [],
    'DELIBERATELY NOT MATCHED: a bare token inside parentheses. `cide-git` has no slash, and a ' +
      'matcher that trusted `Word(token)` would light up every tool line in the transcript',
  )
  eq(
    texts('Update(ProjectSwitcher.tsx)'),
    [],
    'and neither is a bare BASENAME in parentheses, even though a human reads it as a file: ' +
      'resolving one against the index is the rule whose false positives are unbounded',
  )
  eq(
    texts('see @ui/src/App.tsx for the call site'),
    ['ui/src/App.tsx'],
    "and mentions them with an `@`, which is the marker and not the path's first character",
  )
  eq(
    only('ui/src/App.tsx#L12-20'),
    { text: 'ui/src/App.tsx', start: 0, end: 21, line: 12, column: null },
    'the `#L12-20` mention form underlines whole and puts the caret on the first line',
  )

  eq(
    texts("cat: 'ui/src/foo.ts': No such file or directory"),
    ['ui/src/foo.ts'],
    'bash quotes the path it could not find',
  )
  eq(
    texts("cat: 'ui/src/my file.ts': No such file or directory"),
    ['ui/src/my file.ts'],
    'a quoted run is the ONE place a space may be inside a path, because there the producer said where it ended',
  )
  eq(
    texts("don't open src/foo.rs' contents"),
    ['src/foo.rs'],
    "and an English possessive does not open a quoted span — otherwise this line's real path " +
      'would be swallowed by an apostrophe two words earlier',
  )

  eq(
    texts('   Compiling cide-app v0.1.0 (/home/u/work/cide)'),
    ['/home/u/work/cide'],
    'cargo names a DIRECTORY in parentheses; the grammar admits it and stage 3 is what refuses it',
  )

  // --- the negatives, which are the whole point ------------------------------------------

  const nothing = [
    ['0.12.0', 'a version number is not a path'],
    ['v0.1.0', 'nor is a tagged one'],
    ['   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.31s', 'nor `target(s)`'],
    ['e.g. the README file, or Cargo.toml', 'nor any other dotted English word'],
    ['http://localhost:5173/src/main.tsx', 'a URL contributes nothing, insides included'],
    ['file:///home/u/work/cide/run.sh', 'and neither does a file:// URL'],
    ['  ui/src/sidebar/GitPa…', 'a path the TUI truncated is the TUI saying this is not the path'],
    ['  ui/src/sidebar/GitPa...', 'including when it spells the ellipsis in ASCII'],
    ['/etc', 'one segment behind a slash is a word with a slash on it'],
    ['ls -la /', 'and the root directory is not a file link'],
    ['building src/', 'a bare directory reference has one non-empty segment'],
    ['C:\\Users\\me\\file.txt', 'a backslash here is an escape, not a separator'],
    ['error at foo bar.rs:3', 'an unquoted path with a space has no discoverable start'],
    ['~/.claude/.credentials.json', 'and `~` is never expanded — one fewer way out of the project'],
  ]
  for (const [line, what] of nothing) {
    eq(texts(line), [], what)
  }

  eq(
    texts('  Read more in docs/adr/0001.md, e.g. section 2'),
    ['docs/adr/0001.md'],
    'one real path in a sentence of prose yields exactly one candidate',
  )

  eq(
    texts('either/or, this/that'),
    ['either/or', 'this/that'],
    'two slashed words in prose ARE candidates — the grammar is a generator and the index is ' +
      'the oracle; see the `none` assertion below for what the user actually sees',
  )

  eq(
    texts('@xterm/addon-web-links@0.12.0 is declared'),
    ['xterm/addon-web-links@0.12.0'],
    'a scoped package spec looks exactly like a path and is one, until something asks the disk',
  )

  ok(matchPaths('x'.repeat(MAX_LINE + 500)).length === 0, 'an absurd logical line is not scanned past the cap')

  // --- stage 2: which absolute paths could this name? -------------------------------------

  const ROOT = '/home/u/work/cide'
  const bases = (cwds, roots = [ROOT]) => ({ cwds, roots })

  eq(
    candidatePaths('src/App.tsx', bases([`${ROOT}/ui`])),
    [`${ROOT}/ui/src/App.tsx`, `${ROOT}/src/App.tsx`],
    'a relative path is tried against the cwd first and every root after it',
  )
  eq(
    candidatePaths(`${ROOT}/run.sh`, bases([`${ROOT}/ui`])),
    [`${ROOT}/run.sh`],
    'an absolute path is itself and nothing else',
  )
  eq(
    candidatePaths('/etc/shadow', bases([`${ROOT}/ui`])),
    [],
    'an absolute path outside every root is never asked of the INDEX — `candidatePaths` is ' +
      'the in-project set and stays that way; the out-of-project set is its own function',
  )
  eq(
    candidatePaths('../../.ssh/id_rsa', bases([`${ROOT}/ui`])),
    [],
    'and a relative one that climbs out is dropped AFTER normalisation, so the climb cannot ' +
      'be hidden behind a middle segment',
  )

  // --- stage 2b: which paths may be offered as an OUT-OF-PROJECT open? ---------------------
  //
  // The narrow half. Everything here is one ctrl+click plus one confirmation away from a file
  // the project does not contain, so every negative is a security assertion and not a taste.

  eq(
    outsidePaths('/usr/lib/go/src/fmt/print.go', bases([`${ROOT}/ui`])),
    ['/usr/lib/go/src/fmt/print.go'],
    'a fully spelled absolute path outside every root is the case the feature exists for',
  )
  eq(
    outsidePaths(`${ROOT}/run.sh`, bases([`${ROOT}/ui`])),
    [],
    'a path INSIDE a root is never an out-of-project candidate — the two sets are disjoint, ' +
      'which is what stops `resolveCandidate` inventing a `many` out of one file',
  )
  eq(
    outsidePaths('../../.ssh/id_rsa', bases([`${ROOT}/ui`])),
    [],
    'A RELATIVE PATH NEVER LEAVES THE PROJECT. Resolved against the pane cwd this is a real ' +
      'private key, and the user would have approved it having read six characters of it',
  )
  eq(
    outsidePaths('.ssh/id_rsa', bases(['/home/u'])),
    [],
    'and neither does a plain relative one that happens to resolve outside: the whole ' +
      'property is that the full path was on screen before the click',
  )
  eq(
    outsidePaths('/home/u/work/cide/../../.ssh/id_rsa', bases([`${ROOT}/ui`])),
    [],
    'a climbing ABSOLUTE path is refused too — it normalises to something outside, but that ' +
      'string was never written on screen, and Rust refuses `..` however anyone approves it',
  )
  eq(
    outsidePaths('/etc//passwd', bases([ROOT])),
    [],
    'and so is any spelling that is not already canonical, for the same reason: `openable` ' +
      'takes the path as given, so an offered link must be one it will accept',
  )
  eq(
    outsidePaths('~/.claude/.credentials.json', bases([ROOT])),
    [],
    'a tilde is a shell word, not a path; expanding it here would be this module inventing a ' +
      'path nobody printed',
  )
  eq(
    candidatePaths('./run.sh', bases([ROOT])),
    [`${ROOT}/run.sh`],
    '`./` resolves against the cwd and collapses',
  )
  eq(
    candidatePaths('src/App.tsx', bases([ROOT, ROOT])),
    [`${ROOT}/src/App.tsx`],
    'a cwd equal to a root is one candidate, not two — the caller pays one probe',
  )
  eq(
    candidatePaths('src/x.ts', bases([], ['/a', '/b'])),
    ['/a/src/x.ts', '/b/src/x.ts'],
    'a multi-root project offers one candidate per root, in `Project.roots` order',
  )

  // --- stage 3: one, many, or none --------------------------------------------------------

  const ctx = (files, cwds, roots = [ROOT]) => ({
    cwds,
    roots,
    isFile: (p) => files.includes(p),
  })

  eq(
    resolveCandidate('src/App.tsx', ctx([`${ROOT}/ui/src/App.tsx`], [`${ROOT}/ui`])),
    { kind: 'one', path: `${ROOT}/ui/src/App.tsx` },
    'the tsc case: a pane that has cd-ed into ui resolves ui-relative output',
  )
  eq(
    resolveCandidate('crates/cide-app/src/lib.rs', ctx([`${ROOT}/crates/cide-app/src/lib.rs`], [`${ROOT}/ui`])),
    { kind: 'one', path: `${ROOT}/crates/cide-app/src/lib.rs` },
    'and the cargo case still resolves from the same pane, because the roots are tried too',
  )
  eq(
    resolveCandidate('either/or', ctx([], [ROOT])),
    { kind: 'none' },
    'prose that parses as a path names no file, so nothing is ever drawn for it',
  )
  eq(
    resolveCandidate('/home/u/work/cide', ctx([], [ROOT])),
    { kind: 'none' },
    "cargo's `(/abs/dir)` is a directory: `isFile` is false for it and no link is offered",
  )
  eq(
    resolveCandidate(
      'src/x.ts',
      ctx(['/a/src/x.ts', '/b/src/x.ts'], [], ['/a', '/b']),
    ),
    { kind: 'many', paths: ['/a/src/x.ts', '/b/src/x.ts'] },
    'TWO roots hold this file — the answer is `many`, and a caller that opens the first is ' +
      'the bug this whole design exists to prevent',
  )
  eq(
    resolveCandidate(
      'src/App.tsx',
      ctx([`${ROOT}/ui/src/App.tsx`, `${ROOT}/src/App.tsx`], [`${ROOT}/ui`]),
    ),
    { kind: 'many', paths: [`${ROOT}/ui/src/App.tsx`, `${ROOT}/src/App.tsx`] },
    'and a cwd that agrees with a root is ambiguous too: the cwd is read NOW and the line was ' +
      'printed some time ago, so "the cwd wins" is a guess wearing a rule',
  )
  eq(
    resolveCandidate('src/App.tsx', ctx([`${ROOT}/ui/src/App.tsx`], [])),
    { kind: 'none' },
    'with no cwd and no root there is nothing to resolve against, and nothing is invented',
  )
  eq(
    resolveCandidate('/usr/lib/go/src/fmt/print.go', ctx(['/usr/lib/go/src/fmt/print.go'], [])),
    { kind: 'one', path: '/usr/lib/go/src/fmt/print.go' },
    'an out-of-project file that really is there resolves — the click, not the hover, is ' +
      'where "may cide open this" gets asked, and it gets asked of the user',
  )
  eq(
    resolveCandidate('/etc/shadow', ctx([], [])),
    { kind: 'none' },
    'and one that is not a regular file resolves to nothing, so `/dev/null`, `/proc/…` and ' +
      'every `.so` in a linker error stay un-underlined',
  )
  eq(
    resolveCandidate('../../.ssh/id_rsa', ctx(['/home/u/.ssh/id_rsa'], ['/home/u/w/x'], ['/home/u/w/x'])),
    { kind: 'none' },
    'THE ONE THAT MATTERS: a climbing relative path resolves to a real private key on disk ' +
      'and is still offered nothing, because neither candidate set will produce it',
  )

  // --- stage 3b: a directory is an answer too ---------------------------------------------
  //
  // Before M15 a directory resolved to `none`, no link was drawn, and the press was forwarded to
  // the child — which answered a ctrl+click on a folder by opening the desktop file manager. The
  // lesson generalises past this case and is why these assertions exist: **a gesture cide
  // declines is a gesture cide has delegated**, so "no link" was never the neutral outcome it
  // looked like.

  const dirCtx = (dirs, cwds, roots = [ROOT]) => ({
    cwds,
    roots,
    isDir: (p) => dirs.includes(p),
  })

  eq(
    resolveDirectory('crates/cide-app', dirCtx([`${ROOT}/crates/cide-app`], [ROOT])),
    { kind: 'one', path: `${ROOT}/crates/cide-app` },
    'a directory resolves, so a ctrl+click on it can be answered by cide instead of by the child',
  )
  eq(
    resolveDirectory('/home/u/work/cide', dirCtx(['/home/u/work/cide'], [], [ROOT])),
    { kind: 'one', path: '/home/u/work/cide' },
    "cargo's `Compiling … (/abs/dir)` line is the everyday case, and it is a real answer now",
  )
  eq(
    resolveDirectory('src', dirCtx(['/a/src', '/b/src'], [], ['/a', '/b'])),
    { kind: 'many', paths: ['/a/src', '/b/src'] },
    'and two roots holding the same folder is exactly as ambiguous as two holding the same ' +
      'file — the refusal is the same one, because it is literally the same function',
  )
  eq(
    resolveDirectory('../../.ssh', dirCtx(['/home/u/.ssh'], ['/home/l/w/x'], ['/home/l/w/x'])),
    { kind: 'none' },
    'THE ONE THAT MATTERS, again: containment is shared with the file resolver, so a climbing ' +
      'relative path names no directory either. A second copy of this function would be a ' +
      'second place to forget `outsidePaths`',
  )
  eq(
    resolveDirectory('either/or', dirCtx([], [ROOT])),
    { kind: 'none' },
    'prose that parses as a path is still nothing, so no folder underline appears in a sentence',
  )

  // --- the press gate: what a mouse press in a terminal pane means -------------------------
  //
  // THE DEFECT, as a property rather than a line of source. `pressVerdict` is not given the
  // hover state, so it cannot wait for one. The old gate's `if (hovered === null) return` sat
  // BEFORE `preventDefault`, so a press over a Claude pane — where an alt-screen repaint under a
  // stationary pointer means no `mousemove` ever fires and no hover is ever recorded — went
  // straight through to xterm, to the pty, and to `claude`.

  eq(
    pressVerdict({ button: 0, ctrlKey: true, metaKey: false }),
    'claim',
    'a ctrl+left press inside a terminal pane is cide\'s, and the verdict depends on NOTHING ' +
      'else — not on a hover, not on a resolved candidate, not on a probe having answered',
  )
  eq(
    pressVerdict({ button: 0, ctrlKey: false, metaKey: true }),
    'claim',
    'and so is cmd+left, matching how the keymap normalises `mod`',
  )
  eq(
    pressVerdict({ button: 0, ctrlKey: false, metaKey: false }),
    'ignore',
    'a plain left click is the child\'s: it focuses the pane, and under a mouse-tracking TUI it ' +
      'is a click the program is entitled to receive',
  )
  eq(
    pressVerdict({ button: 1, ctrlKey: true, metaKey: false }),
    'ignore',
    'middle-click paste is untouched even with Ctrl held',
  )
  eq(
    pressVerdict({ button: 2, ctrlKey: true, metaKey: false }),
    'ignore',
    'and so is the right button, which the pane opens its own context menu from',
  )
  /*
   * The verdict is a function of the event and of NOTHING else. Three ways, because the obvious
   * one is not enough:
   *
   * `.length === 1` catches a second *required* parameter. It does not catch a defaulted one —
   * `Function.length` counts only the parameters before the first default — and the mutation
   * that proves this matters is exactly the shape a well-meaning edit would take:
   * `pressVerdict(ev, hovered = true)`, which reads as backwards-compatible and reinstates the
   * shipped bug for every caller that passes the second argument.
   *
   * So: an extra argument must make no difference to the answer, and the declaration itself is
   * pinned over comment-stripped source. All three were written after `.length` alone survived
   * its mutation.
   */
  ok(
    pressVerdict.length === 1,
    'pressVerdict takes ONE argument — the event. The moment it takes a second (a hover, a ' +
      'resolution, a cache) the early return that forwarded the gesture to `claude` can come ' +
      'back, and no assertion about today\'s behaviour would notice',
  )
  eq(
    pressVerdict({ button: 0, ctrlKey: true, metaKey: false }, false),
    'claim',
    'and nothing passed alongside the event can change the verdict — a DEFAULTED second ' +
      'parameter is invisible to Function.length and is how `if (!hovered) return` comes back ' +
      'wearing a compatible-looking signature',
  )
  ok(
    /export function pressVerdict\(ev: PressLike\): PressVerdict \{/.test(
      code('src/terminal/clickGate.ts'),
    ),
    'and the declaration says so: exactly one parameter, no default, no optional. This is the ' +
      'whole of M15\'s terminal-click fix expressed as a type — the rule cannot consult a hover ' +
      'because it is never handed one',
  )

  // Pixel → cell. A 10x4 grid of 100x40px cells at the origin, scrolled 7 lines into scrollback.
  const grid = { cols: 10, rows: 4, viewportY: 7 }
  const rect = { left: 0, top: 0, width: 1000, height: 160 }
  eq(
    cellFromPoint({ clientX: 0, clientY: 0 }, rect, grid),
    { x: 1, y: 8 },
    'the top-left pixel is column 1 of the viewport\'s first row, in ILink.range\'s 1-based ' +
      'coordinates and offset by viewportY so scrollback lines up',
  )
  eq(
    cellFromPoint({ clientX: 999, clientY: 159 }, rect, grid),
    { x: 10, y: 11 },
    'and the bottom-right pixel is the last cell — a division that was one out here would make ' +
      'every link openable from every column except its last',
  )
  eq(
    cellFromPoint({ clientX: 250, clientY: 45 }, rect, grid),
    { x: 3, y: 9 },
    'a pixel mid-grid lands in the cell containing it',
  )
  for (const [point, what] of [
    [{ clientX: -1, clientY: 10 }, 'left of the grid'],
    [{ clientX: 1000, clientY: 10 }, 'right of the grid'],
    [{ clientX: 10, clientY: -1 }, 'above the grid'],
    [{ clientX: 10, clientY: 160 }, 'below the last row'],
  ]) {
    eq(
      cellFromPoint(point, rect, grid),
      null,
      `a press ${what} is not a press on the nearest cell — clamping would silently make a ` +
        'click on the scrollbar into a click on the text beside it',
    )
  }
  eq(
    cellFromPoint({ clientX: 5, clientY: 5 }, { left: 0, top: 0, width: 0, height: 0 }, grid),
    null,
    'and a terminal that has not been laid out yet answers null rather than dividing by zero',
  )

  // Which link a cell is in. Transcribed from xterm's own `Linkifier._linkAtPosition`, so the
  // link a press opens is decided by the same predicate that decided which one got underlined.
  const sameLine = { start: { x: 7, y: 3 }, end: { x: 20, y: 3 } }
  ok(linkAtCell(sameLine, { x: 7, y: 3 }), 'the first cell of a link is inside it')
  ok(linkAtCell(sameLine, { x: 20, y: 3 }), 'and so is the last — `end.x` is inclusive')
  ok(!linkAtCell(sameLine, { x: 6, y: 3 }), 'the cell before it is not')
  ok(!linkAtCell(sameLine, { x: 21, y: 3 }), 'nor the cell after it')
  ok(!linkAtCell(sameLine, { x: 10, y: 2 }), 'nor the same column on the line above')

  const wrapped = { start: { x: 70, y: 3 }, end: { x: 12, y: 5 } }
  ok(linkAtCell(wrapped, { x: 79, y: 3 }), 'a wrapped link covers its first row from start.x on')
  ok(!linkAtCell(wrapped, { x: 69, y: 3 }), 'but not the cell before it on that row')
  ok(linkAtCell(wrapped, { x: 1, y: 4 }), 'the whole of every middle row is inside it')
  ok(linkAtCell(wrapped, { x: 79, y: 4 }), 'the whole of it, both ends')
  ok(linkAtCell(wrapped, { x: 12, y: 5 }), 'and its last row up to end.x')
  ok(
    !linkAtCell(wrapped, { x: 13, y: 5 }),
    'and no further — a long absolute path in a narrow pane wraps, so this is the common case ' +
      'rather than the exotic one, and a simpler predicate would disagree with the underline',
  )

  // --- the source pins: each is a fix a later edit would silently undo ---------------------


  const xterm = read('src/terminal/xterm.ts')
  ok(
    /linkHandler\s*:/.test(xterm),
    'xterm.ts sets `linkHandler` — without it xterm 6 answers a click on an OSC 8 hyperlink ' +
      'from ANY program with confirm() + window.open() inside this app\'s own webview',
  )

  const hosts = read('src/layout/paneHosts.ts')
  ok(
    hosts.includes('attachPathLinks('),
    'paneHosts.ts attaches the link provider — this one line is the whole feature\'s only ' +
      'call site, and this project has shipped eleven features reachable from nothing',
  )

  const links = code('src/terminal/pathLinks.ts')
  ok(
    /addEventListener\('mousedown',[^)]*true\)/.test(links),
    'the ctrl+click gate is a CAPTURE listener: xterm\'s own always-on mousedown reports the ' +
      'press to the pty under any mouse-tracking TUI, and only capture runs before it',
  )
  /*
   * The ordering inside the callback, which no pure function can express because it is about
   * *when* a side effect happens relative to a decision. Both halves are needed: the first says
   * the press is taken, the second says it is taken before anything is known about it.
   */
  const gateBody = links.slice(
    links.indexOf('const onMouseDown'),
    links.indexOf('function actOnCell'),
  )
  ok(
    gateBody.includes("pressVerdict(ev) === 'ignore'"),
    'the gate asks clickGate.ts and does not re-derive the rule — a second copy of "which press ' +
      'is ours" is a second place for the hover check to come back',
  )
  ok(
    gateBody.indexOf('ev.stopPropagation()') < gateBody.indexOf('cellUnder(ev)'),
    'THE FIX, as an ordering: the press is swallowed BEFORE anything is resolved. Everything ' +
      'downstream is asynchronous — a cwd probe and an existence probe, both IPC round trips — ' +
      'so a gate that decided first would hand the press to xterm, to the pty and to `claude` ' +
      'in the window between',
  )
  ok(
    !/hovered\s*===\s*null|!hovered\s*\)\s*return/.test(gateBody),
    'and it consults no hover state at all. `if (hovered === null) return` sitting above ' +
      'preventDefault IS the shipped bug: in a Claude pane the alt-screen TUI repaints under a ' +
      'stationary pointer, so no mousemove fires, so no hover is ever recorded, so every ' +
      'ctrl+click was forwarded to the child',
  )
  ok(
    links.includes("kind === 'many'") || links.includes('many'),
    'pathLinks.ts has an arm for the ambiguous answer rather than taking paths[0]',
  )
  ok(
    links.includes('env.open === null'),
    'a pane with nowhere to send an open offers no links at all — an underline that takes a ' +
      'click and swallows it is the exact defect this project keeps finding',
  )
  ok(
    links.includes('env.reveal?.('),
    'and a directory goes to `reveal`, which is `runCommand(\'file.reveal\')` and not a second ' +
      'call into the tree store: the sidebar has to be shown before it is scrolled, and that ' +
      'rule lives in dispatch.ts\'s one arm',
  )
  /*
   * The desktop opener stays out of this file. Two notes on this assertion, both found by
   * mutating it:
   *
   * 1. It used to name only the **Rust** command strings (`fs_show_in_manager`,
   *    `tauri_plugin_opener`), which cannot appear in a frontend module at all — the JS surface
   *    is `fsReveal.showInManager(…)` and `openLogDir()`. Adding a real
   *    `void fsApi.showInManager(path)` to `activate` left it green. The names it looks for are
   *    now the ones a caller here would actually type.
   * 2. Even repaired it is necessary and not sufficient, and that is worth stating because this
   *    pin *passed for a milestone while a file manager was opening on the user's screen*: a
   *    grep over cide cannot see an opener forked by a program cide forwarded the click to. The
   *    press gate above is what makes the gesture cide's in the first place.
   *
   * The three legitimate `tauri_plugin_opener` call sites — the file tree's *Reveal in File
   * Manager* item, *Show project in file manager*, and the settings' *Open log directory* — are
   * deliberately untouched. Each is a control the user pressed that says on its face that it
   * leaves the application. A terminal click says nothing of the sort.
   */
  ok(
    !/showInManager|openLogDir|fs_show_in_manager|openPath|tauri_plugin_opener|window\.open/.test(
      links,
    ),
    'and a path parsed out of terminal bytes reaches a workspace tab or the file tree and ' +
      'nothing else — not the desktop opener, not the file manager, not a new window',
  )

  const client = read('src/ipc/client.ts')
  ok(
    client.includes('terminal_open_path'),
    'the open goes through the containment-checked command, not through raw tab_open_file',
  )

  /*
   * XTVERSION, and why a terminal emulator answering a capability query is a security-shaped
   * assertion rather than a cosmetic one.
   *
   * Claude Code stands down from ctrl+click AND alt+click for xterm.js-family hosts, because
   * they do their own. cide is one and did not say so, so the CLI competed for the chord and
   * answered it by forking a desktop file manager. cide's own gate claims ctrl+click; it
   * deliberately does not claim alt+click (that gesture means something to the child), so this
   * reply is the ONLY thing standing between an alt+click in a Claude pane and `dbus-send`.
   */
  const xtermSrc = code('src/terminal/xterm.ts')
  ok(
    /registerCsiHandler\(\{\s*prefix:\s*'>',\s*final:\s*'q'\s*\}/.test(xtermSrc),
    'xterm.ts answers XTVERSION (CSI > 0 q). @xterm/xterm 6 implements none of it, so without ' +
      'this handler `xtversionName` stays undefined in the child and Claude Code claims ' +
      'ctrl+click and alt+click for its own opener — which for a file: target is dbus-send to ' +
      'org.freedesktop.FileManager1.ShowItems, i.e. the desktop file manager on the parent folder',
  )
  ok(
    xtermSrc.includes("term.input(`\\x1bP>|xterm.js(${XTERM_VERSION})\\x1b\\\\`, false)"),
    'and the reply is DCS > | <name> ST — the shape the CLI\'s own parser wants ' +
      '(/^\\x1bP>\\|(.*?)(?:\\x07|\\x1b\\\\)$/) — sent with wasUserInput=false, because a ' +
      'capability reply is not a keystroke and must not scroll the viewport or clear a selection',
  )
  ok(
    !/TERM_PROGRAM\s*[:=]\s*['"]vscode/.test(xtermSrc),
    'and cide does NOT claim to be VS Code to get the same effect: that variable moves half a ' +
      'dozen unrelated Claude Code behaviours, and announcing xterm.js is simply true',
  )

  const pkgJson = JSON.parse(read('package.json'))
  const declared = /XTERM_VERSION = '([^']+)'/.exec(read('src/terminal/xterm.ts'))
  ok(declared !== null, 'xterm.ts declares XTERM_VERSION as a literal this can read')
  eq(
    declared?.[1],
    pkgJson.dependencies['@xterm/xterm'],
    'and the version cide announces is the version cide runs. A literal invites drift and ' +
      'nothing else in the tree can see both numbers, so this is the one place the claim is ' +
      'held honest',
  )

  const pkg = JSON.parse(read('package.json'))
  ok(
    pkg.dependencies['@xterm/addon-web-links'] === undefined,
    '`@xterm/addon-web-links` stays removed: its default handler is window.open() on a URL ' +
      'parsed out of untrusted bytes, and the part worth having (LinkComputer) is not exported',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('terminal path links: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
