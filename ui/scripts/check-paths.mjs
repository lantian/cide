/**
 * Checks `src/terminal/pathMatch.ts` — the rules deciding what in a pane's output is a file
 * path, and which file on disk it names.
 *
 * Same shape as `check-problems.mjs` and `check-exit-marker.mjs`, and for the same reason:
 * there is no JS test runner in this project, and the module is pure and import-free so the
 * TypeScript in `node_modules` can compile it on its own and node can drive it.
 *
 * # The two failures this exists to prevent
 *
 * 1. **Lighting up prose.** A matcher that offers every dotted token is worse than no feature
 *    at all: it teaches the user that underlines mean nothing. So every negative below is as
 *    load-bearing as every positive, and they are drawn from what this repo's own toolchain
 *    actually prints — `0.12.0`, `v0.1.0`, `target(s)`, a truncated TUI path, a URL.
 * 2. **Opening the wrong file.** A candidate that resolves under two different bases must come
 *    back `many` and must never come back with the first one. That rule has no visible symptom
 *    when it breaks — the link still works, it just sometimes opens somebody else's file — so
 *    it is pinned from both directions here.
 *
 * The positives are transcriptions of real output. Where a shape looked surprising it was run
 * rather than recalled: rustc's `-->`, the trailing colon a Rust panic and esbuild both add,
 * tsc's two entirely different formats, ripgrep's `path:line:` where the third field is
 * content and not a column.
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
   * A bare `tsc` with no tsconfig, deliberately. `pathMatch.ts` imports nothing — not even a
   * type through the `@/*` alias — and if this compile ever needs a tsconfig then something has
   * added an import and the node-testability of the rules has been lost, which is the whole
   * reason they live in their own module.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/pathMatch.ts',
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

  const { matchPaths, candidatePaths, resolveCandidate, MAX_LINE } = await import(
    `file://${join(out, 'pathMatch.js')}`
  )

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
    only('    at f (/home/lantian/work/cide/ui/scripts/check-theme.mjs:42:11)'),
    {
      text: '/home/lantian/work/cide/ui/scripts/check-theme.mjs',
      start: 10,
      end: 66,
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
    texts('   Compiling cide-app v0.1.0 (/home/lantian/work/cide)'),
    ['/home/lantian/work/cide'],
    'cargo names a DIRECTORY in parentheses; the grammar admits it and stage 3 is what refuses it',
  )

  // --- the negatives, which are the whole point ------------------------------------------

  const nothing = [
    ['0.12.0', 'a version number is not a path'],
    ['v0.1.0', 'nor is a tagged one'],
    ['   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.31s', 'nor `target(s)`'],
    ['e.g. the README file, or Cargo.toml', 'nor any other dotted English word'],
    ['http://localhost:5173/src/main.tsx', 'a URL contributes nothing, insides included'],
    ['file:///home/lantian/work/cide/run.sh', 'and neither does a file:// URL'],
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

  const ROOT = '/home/lantian/work/cide'
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
    'an absolute path outside every root is never even asked about',
  )
  eq(
    candidatePaths('../../.ssh/id_rsa', bases([`${ROOT}/ui`])),
    [],
    'and a relative one that climbs out is dropped AFTER normalisation, so the climb cannot ' +
      'be hidden behind a middle segment',
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
    resolveCandidate('/home/lantian/work/cide', ctx([], [ROOT])),
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

  // --- the source pins: each is a fix a later edit would silently undo ---------------------

  const read = (rel) => readFileSync(join(UI, rel), 'utf8')

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

  const links = read('src/terminal/pathLinks.ts')
  ok(
    /addEventListener\('mousedown',[^)]*true\)/.test(links),
    'the ctrl+click gate is a CAPTURE listener: xterm\'s own always-on mousedown reports the ' +
      'press to the pty under any mouse-tracking TUI, and only capture runs before it',
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
    !/fs_show_in_manager|openPath|tauri_plugin_opener|window\.open/.test(links),
    'and a path parsed out of terminal bytes reaches a workspace tab and nothing else — not ' +
      'the desktop opener, not the file manager, not a new window',
  )

  const client = read('src/ipc/client.ts')
  ok(
    client.includes('terminal_open_path'),
    'the open goes through the containment-checked command, not through raw tab_open_file',
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
