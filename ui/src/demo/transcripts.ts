/**
 * What the demo's terminals show: Claude Code screens and a shell, drawn as ANSI.
 *
 * Generated rather than recorded. A recording costs a real `claude` turn — the user's quota —
 * and carries whatever that machine's session happened to contain: paths, a login banner, a
 * half-typed prompt. These are deterministic, fit whatever geometry `session_attach` asks for,
 * and use Claude Code's own visual vocabulary (the `⏺` tool rows, `⎿` results, the `✻` spinner
 * line, the rule-bounded prompt and the mode footer) so they read as the real thing at a glance.
 *
 * Colours: named ANSI colours wherever the real CLI uses them, because the terminal maps those
 * through cide's theme tokens — that is the point of a screenshot of cide's terminal. Truecolor
 * only for Claude's brand orange and the diff backgrounds, which the CLI itself draws in 24-bit
 * and which differ between its dark and light modes.
 */
import type { Theme } from './scenes'

const ESC = '\x1b['
const reset = `${ESC}0m`
const bold = (s: string) => `${ESC}1m${s}${ESC}22m`
const dim = (s: string) => `${ESC}2m${s}${ESC}22m`
const fg = (n: number, s: string) => `${ESC}${n}m${s}${ESC}39m`
const rgb = (r: number, g: number, b: number, s: string) => `${ESC}38;2;${r};${g};${b}m${s}${ESC}39m`
const bg = (r: number, g: number, b: number, s: string) => `${ESC}48;2;${r};${g};${b}m${s}${ESC}49m`

const orange = (s: string) => rgb(215, 119, 87, s)
const green = (s: string) => fg(32, s)
const red = (s: string) => fg(31, s)
const blue = (s: string) => fg(34, s)
const yellow = (s: string) => fg(33, s)
const magenta = (s: string) => fg(35, s)
const cyan = (s: string) => fg(36, s)

/** Cut a line to `cols` visible cells, keeping escapes intact. */
function clip(s: string, cols: number): string {
  let out = ''
  let seen = 0
  for (let i = 0; i < s.length; ) {
    if (s[i] === '\x1b') {
      const end = s.indexOf('m', i)
      out += s.slice(i, end + 1)
      i = end + 1
      continue
    }
    const ch = String.fromCodePoint(s.codePointAt(i) ?? 32)
    if (seen >= cols) break
    out += ch
    seen++
    i += ch.length
  }
  return out + reset
}

type Conversation = 'feature' | 'tests' | 'review' | 'refactor'

interface Turn {
  prompt: string
  body: (t: Theme) => string[]
  spinner: string
}

type Rgb = [number, number, number]
const addBg = (t: Theme): Rgb => (t === 'dark' ? [34, 60, 38] : [218, 245, 222])
const delBg = (t: Theme): Rgb => (t === 'dark' ? [74, 34, 38] : [253, 223, 225])
const added = (t: Theme, n: number, code: string) => bg(...addBg(t), `  ${String(n).padStart(4)} ${green('+')} ${code}`)
const removed = (t: Theme, n: number, code: string) => bg(...delBg(t), `  ${String(n).padStart(4)} ${red('-')} ${code}`)
const ctx = (n: number, code: string) => dim(`  ${String(n).padStart(4)}   `) + code

const TURNS: Record<Conversation, Turn> = {
  feature: {
    prompt: 'Split the PTY coalescer out of session.rs and cover backpressure with a test',
    spinner: 'Coalescing… (38s · ↓ 2.4k tokens · esc to interrupt)',
    body: (t) => [
      `${fg(37, '⏺')} I'll read how frames are batched today before moving anything.`,
      '',
      `${green('⏺')} ${bold('Read')}(crates/cide-pty/src/session.rs)`,
      `  ⎿  Read ${bold('412')} lines`,
      '',
      `${green('⏺')} ${bold('Search')}(pattern: "fn flush", path: "crates/cide-pty")`,
      '  ⎿  Found 3 files',
      '',
      `${green('⏺')} ${bold('Update')}(crates/cide-pty/src/coalesce.rs)`,
      `  ⎿  Updated ${bold('crates/cide-pty/src/coalesce.rs')} with ${bold('38')} additions and ${bold('4')} removals`,
      ctx(41, `${magenta('impl')} Coalescer {`),
      removed(t, 42, `    ${magenta('pub fn')} ${blue('push')}(&${magenta('mut')} self, bytes: &[u8]) {`),
      added(t, 42, `    ${magenta('pub fn')} ${blue('push')}(&${magenta('mut')} self, bytes: &[u8]) -> ${yellow('Pressure')} {`),
      added(t, 43, `        ${magenta('if')} self.unacked > self.window {`),
      added(t, 44, `            ${magenta('return')} ${yellow('Pressure')}::Hold;`),
      added(t, 45, '        }'),
      ctx(46, '        self.pending.extend_from_slice(bytes);'),
      '',
      `${green('⏺')} ${bold('Bash')}(cargo test -p cide-pty coalesce)`,
      `  ⎿  test coalesce::holds_back_past_the_window ... ${green('ok')}`,
      `     test coalesce::releases_on_ack ... ${green('ok')}`,
      `     test result: ${green('ok')}. 14 passed; 0 failed`,
      '',
      `${fg(37, '⏺')} The coalescer now lives in ${cyan('coalesce.rs')}, returns ${cyan('Pressure::Hold')}`,
      '  past the ack window, and both new tests pass. Opening the diff in cide.',
    ],
  },
  tests: {
    prompt: 'Run the workspace tests and fix anything red',
    spinner: 'Testing… (1m 12s · ↓ 5.1k tokens · esc to interrupt)',
    body: () => [
      `${green('⏺')} ${bold('Bash')}(cargo test --workspace)`,
      `  ⎿  test result: ${green('ok')}. 612 passed; 0 failed; 9 ignored`,
      `     test result: ${red('FAILED')}. 87 passed; 1 failed`,
      `     ${red('---- layout::moves_a_pane_between_rows stdout ----')}`,
      '',
      `${green('⏺')} ${bold('Read')}(crates/cide-core/src/layout.rs)`,
      `  ⎿  Read ${bold('880')} lines`,
      '',
      `${fg(37, '⏺')} The ratio is kept from the old parent. Fixing that.`,
      '',
      `${green('⏺')} ${bold('Update')}(crates/cide-core/src/layout.rs)`,
      `  ⎿  Updated with ${bold('3')} additions and ${bold('1')} removal`,
      '',
      `${green('⏺')} ${bold('Bash')}(cargo test -p cide-core layout)`,
      `  ⎿  test result: ${green('ok')}. 88 passed; 0 failed`,
    ],
  },
  review: {
    prompt: '/review the open merge request',
    spinner: 'Reviewing… (22s · ↓ 1.8k tokens)',
    body: () => [
      `${green('⏺')} ${bold('cide')} - mr_review (MCP)(iid: 482)`,
      `  ⎿  3 files, 214 lines, pipeline ${green('passed')}`,
      '',
      `${fg(37, '⏺')} Two findings worth a comment, drafted for you to publish:`,
      `  ${yellow('major')}  retry loop ignores the cancellation token`,
      `  ${blue('minor')}  the new setting is missing from the docs table`,
    ],
  },
  refactor: {
    prompt: 'Rename SessionSink to Sink across the workspace',
    spinner: 'Renaming… (9s · ↓ 640 tokens)',
    body: () => [
      `${green('⏺')} ${bold('cide')} - find_usages (MCP)(symbol: "SessionSink")`,
      '  ⎿  41 usages in 12 files',
      '',
      `${green('⏺')} ${bold('Update')}(crates/cide-pty/src/sink.rs)`,
      `  ⎿  Updated with ${bold('6')} additions and ${bold('6')} removals`,
    ],
  },
}

function header(cols: number): string[] {
  const logo = [' ▐▛███▜▌ ', '▝▜█████▛▘', '  ▘▘ ▝▝  ']
  const text = [
    `${bold('Claude Code')} ${dim('v2.1.247')}`,
    dim('Opus 5 (1M context) · Claude Max'),
    dim('~/work/cide'),
  ]
  return logo.map((l, i) => clip(`${orange(l)}  ${text[i] ?? ''}`, cols))
}

/** One Claude Code screen, laid out for exactly `cols` × `rows`. */
export function claudeScreen(kind: Conversation, theme: Theme, cols: number, rows: number): string {
  const turn = TURNS[kind]
  const rule = dim('─'.repeat(cols))
  const top = [...header(cols), '', `${dim('>')} ${turn.prompt}`, '', ...turn.body(theme), '', `${orange('✻')} ${orange(turn.spinner.split(' ')[0] ?? '')} ${dim(turn.spinner.split(' ').slice(1).join(' '))}`]
  const bottom = [rule, `${bold('>')} ${ESC}7m ${ESC}27m`, rule, `  ${orange('⏵⏵ auto mode on')} ${dim('(shift+tab to cycle)')}`]
  const room = Math.max(0, rows - bottom.length)
  // Keep the header and the prompt, and drop from the conversation's top when the pane is short
  // — a screen that has scrolled its own question away reads as a log, not as a conversation. A
  // pane too short for even that (a grid squeezed above a tool window) loses the header instead:
  // three rows of logo fragments over an input box is the worst of both.
  const keep = room >= 16 ? 6 : 0
  const head = room >= 16 ? top : top.slice(4)
  const body = head.length > room ? [...head.slice(0, keep || 2), ...head.slice(head.length - (room - (keep || 2)))] : head
  const lines = [...body, ...Array<string>(Math.max(0, room - body.length)).fill(''), ...bottom]
  return lines.map((l) => clip(l, cols)).join('\r\n')
}

/** A shell that has just run the tests. */
export function shellScreen(cols: number, rows: number): string {
  const prompt = `${green('dev@cide')}:${blue('~/work/cide')}$ `
  const lines = [
    `${prompt}cargo test -p cide-pty`,
    `   ${green(bold('Compiling'))} cide-pty v0.9.1 (/home/dev/work/cide/crates/cide-pty)`,
    `    ${green(bold('Finished'))} \`test\` profile [unoptimized + debuginfo] in 4.21s`,
    `     ${green(bold('Running'))} unittests src/lib.rs`,
    '',
    'running 14 tests',
    `test coalesce::holds_back_past_the_window ... ${green('ok')}`,
    `test coalesce::releases_on_ack ... ${green('ok')}`,
    `test session::a_detach_is_gapless ... ${green('ok')}`,
    `test vt::mirror_tracks_alternate_screen ... ${green('ok')}`,
    '',
    `test result: ${green('ok')}. 14 passed; 0 failed; 0 ignored; finished in 0.38s`,
    '',
    `${prompt}git status --short`,
    ` ${red('M')} crates/cide-pty/src/coalesce.rs`,
    ` ${red('M')} crates/cide-pty/src/session.rs`,
    `${prompt}${ESC}7m ${ESC}27m`,
  ]
  const shown = lines.slice(Math.max(0, lines.length - rows))
  return shown.map((l) => clip(l, cols)).join('\r\n')
}

export type TerminalContent = { kind: 'claude'; conversation: Conversation } | { kind: 'shell' }

export function screen(content: TerminalContent, theme: Theme, cols: number, rows: number): string {
  return content.kind === 'claude' ? claudeScreen(content.conversation, theme, cols, rows) : shellScreen(cols, rows)
}

export const encode = (s: string): ArrayBuffer => new TextEncoder().encode(s).buffer as ArrayBuffer
