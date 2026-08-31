/**
 * The JSON log line's hyperlink: its URI, and the wiring that makes clicking one do anything.
 *
 * `terminal/logLink.ts` is import-free so this can compile and drive it standalone. What it
 * parses arrives from a *program's output* — any child can emit an OSC 8 sequence naming any
 * URI it likes, this scheme included — so the parser is the boundary and its refusals are the
 * specification.
 *
 * The rest of this file is source-text assertions, because the pieces they hold together
 * cannot import one another: the scheme is spelled in Rust and in TypeScript, the handler that
 * receives the URI is in a module that only mounts under a real DOM, and the card has to be
 * mounted in *both* branches of `App.tsx` — a card wired only into the shell tree leaves the
 * click in a detached pane doing nothing at all, which is a defect this repo has shipped twice
 * in other clothes.
 *
 * Run: `pnpm --dir ui run check:json-log`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
let failed = 0
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a === b) return
  failed += 1
  console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => eq(cond === true, true, what)

const out = mkdtempSync(join(tmpdir(), 'cide-json-log-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/logLink.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const link = await import(`file://${join(out, 'logLink.js')}`)

  const session = '4a7c2f10-9b3d-4e6a-8f21-0c5d7e9a1b34'
  const uri = link.formatLogLink(session, 42)
  eq(uri, `cide-log:${session}:42`, 'the URI is the scheme, the session and the handle')
  eq(link.parseLogLink(uri), { session, handle: 42 }, 'and it round-trips through the parser')
  eq(link.parseLogLink(link.formatLogLink(session, 0)), { session, handle: 0 },
    'handle 0 is the first line of a session and must not be read as absent')
  ok(link.isLogLink(uri), 'isLogLink agrees with the parser')

  /*
   * The refusals. Each of these is something a child process can put on the screen by writing
   * its own OSC 8 sequence, and the parser's answer is what stops it being sent to a command.
   */
  for (const bad of [
    'https://example.com',
    'file:///etc/passwd',
    'javascript:alert(1)',
    'cide-log:',
    `cide-log:${session}`,
    `cide-log:${session}:`,
    `cide-log:${session}:abc`,
    // Number() would take every one of these; a handle is decimal digits and nothing else.
    `cide-log:${session}: 3`,
    `cide-log:${session}:0x2`,
    `cide-log:${session}:1e3`,
    `cide-log:${session}:-1`,
    `cide-log:${session}:9007199254740993`,
    // A session that is not a uuid — the string goes straight to a command, so its shape is
    // checked here rather than trusted.
    'cide-log:../../etc:1',
    'cide-log:{}:1',
    `cide-log:${session}x:1`,
    // The scheme has to match at the start, not anywhere.
    `x-cide-log:${session}:1`,
  ]) {
    eq(link.parseLogLink(bad), null, `refuses ${JSON.stringify(bad)}`)
    eq(link.isLogLink(bad), false, `isLogLink refuses ${JSON.stringify(bad)}`)
  }

  // --- the cross-language and cross-module wiring -------------------------------------------

  const rust = readFileSync(resolve(UI, '..', 'crates', 'cide-app', 'src', 'lifecycle.rs'), 'utf8')
  const declared = /LOG_LINK_SCHEME: &str = "([^"]+)"/.exec(rust)?.[1] ?? null
  eq(declared, link.LOG_LINK_SCHEME,
    'Rust writes the scheme this module parses — the two cannot import one another, and a '
      + 'rename on either side is a link that silently stops opening anything')
  ok(/format!\("\{LOG_LINK_SCHEME\}:\{session\}:\{handle\}"\)/.test(rust),
    'and Rust assembles it in the order the parser reads it')

  const jsonlog = readFileSync(resolve(UI, '..', 'crates', 'cide-core', 'src', 'jsonlog.rs'), 'utf8')
  ok(/fn osc8\(uri: &str\)/.test(jsonlog) && jsonlog.includes('OSC8_END'),
    'the marker is OSC 8, which is what makes it occupy no cells and survive a wrapped line')

  const xterm = readFileSync(join(UI, 'src', 'terminal', 'xterm.ts'), 'utf8')
  ok(/allowNonHttpProtocols:\s*true/.test(xterm),
    'the link handler accepts non-http URIs — at the default of false, xterm’s OSC link '
      + 'provider drops `cide-log:` before any handler is asked and the click does nothing')
  ok(xterm.includes('parseLogLink(uri)') && xterm.includes('showLogDetail('),
    'and it routes a parsed log link to the card rather than to the web-link refusal')
  ok(/notify\(`cide does not open web links/.test(xterm),
    'while every other URI still meets the refusal: allowing non-http protocols through to '
      + 'this handler must not become allowing cide to open them')

  const app = readFileSync(join(UI, 'src', 'App.tsx'), 'utf8')
  eq((app.match(/<LogDetailCard \/>/g) ?? []).length, 2,
    'the card is mounted in BOTH branches of App.tsx — the shell window and the '
      + 'pane:<uuid> window. A detached pane renders log lines exactly as a docked one does, '
      + 'and one mount means the click in it asks nobody and shows nothing')

  const store = readFileSync(join(UI, 'src', 'chrome', 'logDetailStore.ts'), 'utf8')
  ok(/state\.pending === null \? state :/.test(store),
    'a lookup that lands after the card was dismissed does not reopen it')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`json-log: ok (scheme ${link.LOG_LINK_SCHEME}, parser + wiring)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
