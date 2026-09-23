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

// Rust source with its comments taken out, for every assertion below that reads one. Mandatory
// rather than tidy in this file: the house style is to name the failure a rule prevents, so
// `render.rs`'s prose spells the thought row, the tail and the budget that went away, and
// `TokenUsage`'s doc spells the sum its arithmetic is pinned against — every one of those
// assertions would otherwise pass by matching the documentation of the thing it is checking.
const stripRust = (source) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

const out = mkdtempSync(join(tmpdir(), 'cide-json-log-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/logLink.ts',
      'src/terminal/runLinks.ts',
      'src/chrome/logDetailModel.ts',
      // Two source directories: without a stated root, tsc picks their common ancestor and
      // the emitted paths below move with whichever file is added next.
      '--rootDir', 'src',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const link = await import(`file://${join(out, 'terminal', 'logLink.js')}`)

  const session = '4a7c2f10-9b3d-4e6a-8f21-0c5d7e9a1b34'
  const uri = link.formatLogLink(session, 42)
  eq(uri, `cide-log:${session}:42`, 'the URI is the scheme, the session and the handle')
  eq(link.parseLogLink(uri), { session, handle: 42 }, 'and it round-trips through the parser')
  eq(link.parseLogLink(link.formatLogLink(session, 0)), { session, handle: 0 },
    'handle 0 is the first line of a session and must not be read as absent')
  ok(link.isLogLink(uri), 'isLogLink agrees with the parser')

  /*
   * An agent run's tool line (M42): the glyph, the tool, whatever the renderer put between, and
   * the handle token at the very end — `cide_agents::harness::opencode::render_tool`'s shape,
   * read back off the buffer text so a click can resolve it after a replay dropped every OSC 8.
   */
  const run = await import(`file://${join(out, 'terminal', 'runLinks.js')}`)
  const tool = '● bash  cargo test --workspace  1.2s #7'
  eq(
    run.parseRunLine(tool),
    { tool: 'bash', handle: 7, toolEnd: 6, tokenStart: tool.length - 2, end: tool.length },
    'a completed call: the prefix ends after the tool, the token is the last two characters',
  )
  eq(run.parseRunLine(tool + '   ').handle, 7, 'trailing cells are trimmed before the token is read')
  eq(run.parseRunLine('✗ bash  cat ~/.cargo/config.toml  #9').handle, 9, 'a failed call links too')
  eq(run.parseRunLine('● cide_task_get  t-14  8ms #0').handle, 0, 'handle 0 is a line, not absence')
  eq(run.parseRunLine('● bash  cargo test'), null, 'no token, no link — a rendering without a ring')
  eq(run.parseRunLine('#7 ● bash  cargo test'), null, 'the token is read off the end only')
  eq(run.parseRunLine('  The task is already in doing. #7'), null, 'prose ending in a hash is not a tool line')
  eq(run.parseRunLine('● bash  echo #7 #x'), null, 'and the token must be digits')

  /*
   * The thought row (M62). A block of the model's reasoning is one collapsed row whose whole
   * text lives behind the handle, so the row *must* parse: a `∴` line that this grammar refuses
   * shows a `#8` nobody can click, and nothing in Rust or in the pane can see that. Its `tool`
   * is the word `thought`, which is why no second grammar was needed.
   */
  const thought = '∴ thought  4.1s #8'
  eq(
    run.parseRunLine(thought),
    { tool: 'thought', handle: 8, toolEnd: 9, tokenStart: thought.length - 2, end: thought.length },
    'a thought row parses as a run line, prefix and token alike',
  )
  eq(run.parseRunLine('∴ thought  #8').handle, 8, 'and with no duration, which is the unknown case')
  eq(run.parseRunLine('∴ thought'), null, 'no token, no link — a rendering without a ring')

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

  /*
   * The card's reading of a kept line (`chrome/logDetailModel.ts`), and the clock it prints.
   *
   * A tool call opened from a run's pane said what ran and how it ended and not *when* — the
   * meta row had a status and a duration and no date or time at all. The clock has two sources
   * and the card must pick the right one per harness: opencode records the call's own start,
   * codex records nothing, and a codex call therefore falls back to the moment the line reached
   * cide (`LogLineDetail::recordedUnixMs`, stamped by `logring` on arrival). Driven over each
   * harness's real event shape, because the fields are two wire formats that change without
   * telling this file.
   */
  const model = await import(`file://${join(out, 'chrome', 'logDetailModel.js')}`)
  const opencodeTool = JSON.stringify({
    type: 'tool_use',
    sessionID: 's',
    part: {
      type: 'tool',
      tool: 'bash',
      state: {
        status: 'completed',
        input: { command: 'cargo test --workspace' },
        output: 'ok',
        title: 'cargo test --workspace',
        time: { start: 1_758_204_207_000, end: 1_758_204_208_200 },
        metadata: { exit: 0 },
      },
    },
  })
  const opencodeView = model.viewOf(opencodeTool)
  eq(opencodeView.kind, 'tool', 'an opencode tool_use is the tool view')
  eq(opencodeView.startedAt, 1_758_204_207_000,
    'and its clock is the call’s own start, by the harness’s clock — not the line’s arrival, '
      + 'which is a duration later')
  eq(opencodeView.duration, '1.2s', 'the duration beside it is the renderer’s shape')
  eq(opencodeView.command, 'cargo test --workspace', 'one string input is the command')

  const codexCommand = JSON.stringify({
    type: 'item.completed',
    item: {
      id: 'item_1',
      type: 'command_execution',
      command: 'cargo build',
      aggregated_output: '   Compiling x\n',
      exit_code: 0,
      status: 'completed',
    },
  })
  const codexView = model.viewOf(codexCommand)
  eq(codexView.kind, 'tool', 'a codex command_execution is the tool view')
  eq(codexView.startedAt, null,
    'codex records no clock on an item, so the view says so and the card prints the line’s '
      + 'arrival instead — a fabricated start here would be a wrong time drawn confidently')
  eq(model.viewOf(JSON.stringify({
    type: 'item.completed',
    item: { id: 'i', type: 'mcp_tool_call', server: 'cide', tool: 'cide_task_get',
      status: 'completed', arguments: { id: 't-14' } },
  })).startedAt, null, 'an MCP call has no clock either')
  eq(model.viewOf(JSON.stringify({
    type: 'item.completed',
    item: { id: 'i', type: 'file_change', status: 'completed', changes: [] },
  })).startedAt, null, 'nor a file change')
  eq(model.viewOf(JSON.stringify({ type: 'text', part: { type: 'text', text: 'hi' } })),
    { kind: 'text', text: 'hi' }, 'the model’s own words are the text view')

  /*
   * Thinking is its own view on both roads, and the text arrives whole. Since M62 the run's pane
   * draws none of the reasoning — only `∴ thought  4.1s #8` — so this card is the only copy, and
   * a `clip` or a one-line collapse leaking in here would be invisible until somebody opened
   * one. The newlines are part of the claim.
   */
  const thinking = '**Planning**\n\nRead the task first, then the build.'
  eq(model.viewOf(JSON.stringify({ type: 'reasoning', part: { type: 'reasoning', text: thinking } })),
    { kind: 'reasoning', text: thinking },
    'an opencode reasoning part is the reasoning view, unclipped and with its newlines')
  eq(model.viewOf(JSON.stringify({
    type: 'item.completed',
    item: { id: 'item_0', type: 'reasoning', text: thinking },
  })), { kind: 'reasoning', text: thinking }, 'and so is a codex reasoning item')
  eq(model.viewOf('{"level":"info","msg":"tick"}'), null,
    'a structured log line is neither, and stays the JSON view')
  eq(model.viewOf('not json'), null, 'and so does a line that is not JSON')

  // The stamp: a date and a 24-hour clock, local time, always nineteen characters. The
  // instant is built from local components so the assertion holds in every time zone the
  // check runs in; the afternoon hour is what pins the 24-hour form, because `02:03:27` under
  // a 12-hour clock is a different, wrong, and plausible-looking answer.
  const afternoon = new Date(2026, 8, 18, 14, 3, 27).getTime()
  eq(model.stamp(afternoon), '2026-09-18 14:03:27', 'a date and a 24-hour clock, zero-padded')
  eq(model.stamp(new Date(2026, 0, 5, 0, 0, 0).getTime()), '2026-01-05 00:00:00',
    'midnight is 00, not 12, and single digits are padded')
  eq(model.stamp(Number.NaN).length, 19, 'a broken value is a placeholder of the same width')
  ok(!model.stamp(Number.NaN).includes('NaN'), 'and never spells NaN')

  const card = readFileSync(join(UI, 'src', 'chrome', 'LogDetailCard.tsx'), 'utf8')
  ok(/at=\{view\.startedAt \?\? recordedAt\}/.test(card),
    'the tool view draws the harness’s start where there is one and the line’s arrival where '
      + 'there is not — both, in that order, or a codex call has no clock at all again')
  ok(/Number\(pending\.detail\.recordedUnixMs\)/.test(card),
    'and the arrival is converted from the wire’s bigint before any arithmetic can throw '
      + 'inside the render')

  /*
   * Who ran the line, on what, and how full the context is. (M80)
   *
   * Three numbers and a join, every one of which is wrong in a way nobody can see from the
   * screen: the two CLIs count a cached prompt differently, a percentage needs a window cide
   * usually does not have, and a figure grouped one way here and another way elsewhere is two
   * numbers on one card. So the formatters are pure and driven here, and the arithmetic is
   * pinned against the Rust it restates.
   */
  eq(model.groupDigits(5_725), '5,725', 'grouped in threes from the right')
  eq(model.groupDigits(812), '812', 'and not before there is a group to make')
  eq(model.groupDigits(1_000_000), '1,000,000', 'every three, not only the first')
  eq(model.groupDigits(0), '0', 'zero is a number and renders as one')
  ok(!model.groupDigits(Number.NaN).includes('NaN'),
    'a broken value is a placeholder — `stamp`’s rule, for the same reason')

  const spend = { input: 4_000, output: 300, reasoning: 200, cacheRead: 1_000, cacheWrite: 64 }
  eq(model.contextTokens(spend), 5_500,
    'the window is the prompt (cached or not) plus what the model produced — and never the '
      + 'cache *write*, which went to the provider and not into this conversation')
  eq(model.contextLine(spend, null), '5,500 tokens',
    'with no stated window there is no percentage: a percentage of a guessed limit reads as a '
      + 'measurement, which is worse than no number at all')
  eq(model.contextLine(spend, 32_768), '5,500 of 32,768 tokens · 17%',
    'and with one, the share, rounded to whole percent')
  eq(model.contextLine(spend, 0), '5,500 tokens',
    '`0` is opencode’s “write no limit”, so it is an absence here too and not a division by it')
  eq(model.spendLine(spend), '4,000 prompt · 1,000 cached · 300 written · 200 thinking · 64 cache write',
    'the breakdown names each part it has')
  eq(model.spendLine({ input: 1_600, output: 412, reasoning: 0, cacheRead: 4_096, cacheWrite: 0 }),
    '1,600 prompt · 4,096 cached · 412 written',
    'codex reports no reasoning and no cache writes, so it draws three parts rather than five — '
      + 'two of them zeroes claiming to be measurements')
  eq(model.spendLine({ input: 0, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0 }),
    'nothing measurable',
    'a step that spent nothing still answers a sentence: it is a different claim from a run '
      + 'that has never reported one, and the card draws them differently')

  // The same arithmetic in the other language. `TokenUsage::context` is what every consumer in
  // Rust reads and `contextTokens` is what the card draws; they cannot import one another, and
  // a term added on one side only would be a figure that disagrees with itself between the
  // panel and the card.
  const usageRust = stripRust(readFileSync(
    resolve(UI, '..', 'crates', 'cide-ipc', 'src', 'agents.rs'), 'utf8'))
  ok(/self\.input \+ self\.cache_read \+ self\.output \+ self\.reasoning/.test(usageRust),
    'Rust’s `TokenUsage::context` sums the same four fields this module’s `contextTokens` does')
  ok(!/self\.cache_write/.test(usageRust),
    'and neither side counts the cache write, which is not in the window')

  ok(/Number\(run\.usage\.cacheRead\)/.test(card),
    'the card converts every token count off the wire’s bigint before the formatters see it — '
      + '`recordedUnixMs`’s rule, and its reason: arithmetic against a number throws in render')
  ok(/data-audit="logRunFooter"/.test(card),
    'the footer is findable by a render check, on a wrapper element rather than on a component '
      + 'whose props a hyphenated attribute would silently pass through and drop')
  ok(/the CLI's own default/.test(card),
    'a run cide named no model for says so in words: a blank beside a label reads as a dialog '
      + 'that half failed — `TaskDetailPending`’s rule')
  ok(/has not reported a spend/.test(card),
    'and so does a run with no figures yet, for the same reason')

  // --- the cross-language and cross-module wiring -------------------------------------------

  const rust = readFileSync(resolve(UI, '..', 'crates', 'cide-app', 'src', 'lifecycle.rs'), 'utf8')
  const declared = /LOG_LINK_SCHEME: &str = "([^"]+)"/.exec(rust)?.[1] ?? null
  eq(declared, link.LOG_LINK_SCHEME,
    'Rust writes the scheme this module parses — the two cannot import one another, and a '
      + 'rename on either side is a link that silently stops opening anything')
  ok(/format!\("\{LOG_LINK_SCHEME\}:\{session\}:\{handle\}"\)/.test(rust),
    'and Rust assembles it in the order the parser reads it')

  /*
   * The thought row is two literals in two languages with no compiler between them, which is
   * how a click quietly stops doing anything. So the row Rust builds is read out of its source
   * and driven through the parser above — `LOG_LINK_SCHEME`'s rule, one row over.
   */
  const render = stripRust(readFileSync(
    resolve(UI, '..', 'crates', 'cide-agents', 'src', 'harness', 'render.rs'), 'utf8'))
  const row = /format!\("\{DIM\}(.+?)\{RESET\}\{\}", tail\(ms, handle\)\)/.exec(render)?.[1] ?? null
  ok(row !== null, 'render.rs still builds the thought row from one format string')
  ok(run.parseRunLine(`${row}  4.1s #8`) !== null && run.parseRunLine(`${row}  #8`) !== null,
    `the row Rust spells (${row}) is a row this parser reads, with and without a duration`)
  ok(/pub\(super\) fn tail\(ms: Option<u64>, handle: Option<u64>\)/.test(render),
    'and one function spells the `#handle` tail for both harnesses, so a tool line and a '
      + 'thought row cannot drift out of that grammar separately')
  ok(!/REASONING_BUDGET/.test(render),
    'the clipped reasoning line is gone — the collapse is the feature, and a budget here would '
      + 'mean the prose came back')

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
  console.log(`json-log: ok (scheme ${link.LOG_LINK_SCHEME}, parser + card model + wiring)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
