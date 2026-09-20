/**
 * Clickable task codes in terminal output: the matcher, the gate, the spans and the wiring. (M60)
 *
 * `terminal/taskLinks.ts` and `terminal/bufferSpan.ts` are import-free so this can compile and
 * drive them standalone — and the compile is itself the pin, because an `@/…` import makes `tsc`
 * fail here. What they are given is a *program's output*: any child can print anything, so the
 * matcher's refusals are the specification and the negatives below are the whole point. Two of
 * them are not taste but a wrong card: `t-5034` read as `t-503`, and a code sliced at the line
 * cap, each of which links to a real and different task.
 *
 * The rest is source-text assertions, because the pieces they hold together cannot import one
 * another: the prefix is minted in Rust, the provider order lives in a third file, and the
 * refusal sentences must exist in exactly one place now that two surfaces raise this gesture.
 *
 * **Every grep here runs over comment-stripped source.** That is mandatory rather than tidy in
 * this repository: the house style is to name the failure a rule prevents, so the words this
 * file looks for are exactly the words its own explanations contain, and three assertions have
 * already passed by matching their own prose.
 *
 * Run: `pnpm --dir ui run check:task-links`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const ROOT = resolve(UI, '..')
let failed = 0
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a === b) return
  failed += 1
  console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => eq(cond === true, true, what)

/**
 * Source with comments removed. String-aware, so a `'//'` inside a literal does not eat the rest
 * of the line — which matters twice over here, because several sentences being grepped for
 * contain quotes and slashes.
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

const src = (...parts) => readFileSync(resolve(...parts), 'utf8')
const code = (...parts) => stripComments(src(...parts))

const out = mkdtempSync(join(tmpdir(), 'cide-task-links-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/taskLinks.ts',
      'src/terminal/bufferSpan.ts',
      // Compiled here too, not to re-test `parseRunLine` — `check:json-log` owns that — but so
      // the two matchers can be driven against the one line that contains both.
      'src/terminal/runLinks.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const { matchTaskCodes, offeredTaskCodes, MAX_LINE, READY } =
    await import(`file://${join(out, 'taskLinks.js')}`)
  const { logicalLine, spanOf, MAX_SPAN_ROWS } =
    await import(`file://${join(out, 'bufferSpan.js')}`)
  const { parseRunLine } = await import(`file://${join(out, 'runLinks.js')}`)

  const ids = (line) => matchTaskCodes(line).map((c) => c.id)

  // --- the positives: what an agent, a shell and a renderer actually print -------------------

  eq(matchTaskCodes('t-503'), [{ id: 't-503', start: 0, end: 5 }], 'the bare code, whole line')
  eq(matchTaskCodes('t-1'), [{ id: 't-1', start: 0, end: 3 }], 'one digit is the minimum')
  eq(matchTaskCodes('working on t-503 now'), [{ id: 't-503', start: 11, end: 16 }],
    'mid-sentence, with the offsets the span arithmetic depends on')
  eq(matchTaskCodes('done: t-503'), [{ id: 't-503', start: 6, end: 11 }], 'at the end of a line')

  for (const [line, what] of [
    ['(t-503)', 'parenthesised'],
    ['[t-503]', 'in brackets'],
    ['{t-503}', 'in braces'],
    ['<t-503>', 'in angle brackets'],
    ["'t-503'", 'single-quoted'],
    ['"t-503"', 'double-quoted'],
    ['`t-503`', 'backticked'],
    ['{"id":"t-503"}', 'a JSON field, which is how an MCP answer prints one'],
    ['t-503,', 'comma'],
    ['t-503;', 'semicolon'],
    ['t-503:', 'colon'],
    ['t-503!', 'exclamation'],
    ['t-503?', 'question mark'],
    ['Finished t-503.', 'a sentence-final period — the commonest shape in agent prose'],
    ['t-503...', 'an ellipsis is not an extension'],
    ['● t-503', 'a bullet glyph'],
    ['✗ t-503', 'a failure glyph'],
    ['→ t-503', 'an arrow'],
    ['task=t-503', 'after an equals sign'],
    ['--task t-503', 'a flag value'],
    ['#t-503', 'after a hash'],
    ['@t-503', 'after an at sign'],
    ['Ω t-503', 'after a non-ASCII character and a space'],
  ]) {
    eq(ids(line), ['t-503'], `finds it ${what}: ${JSON.stringify(line)}`)
  }

  eq(ids('t-0'), ['t-0'], 'id zero is an id')
  eq(ids('t-0007'), ['t-0007'],
    'leading zeros are legal in a hand-edited tracker; the board decides, not the matcher')
  eq(ids('t-123456789'), ['t-123456789'], 'nine digits is the maximum and it matches')
  eq(ids('t-5034'), ['t-5034'],
    'a longer id matches AS ITSELF — the failure this guards is reading it as t-503, which is a '
      + 'real and different task, so the click opens the wrong card and looks right doing it')
  eq(matchTaskCodes('t-1 and t-2'), [
    { id: 't-1', start: 0, end: 3 },
    { id: 't-2', start: 8, end: 11 },
  ], 'two per line, in order')
  eq(ids('t-1,t-2'), ['t-1', 't-2'], 'adjacency: the separator is never eaten by the first match')
  eq(ids('t-5 t-5'), ['t-5', 't-5'], 'a duplicate is two links, because it is two runs of cells')

  // --- the negatives, which are the whole point ----------------------------------------------

  for (const line of [
    // Inside a word. `gpt-5` and `rust-1.89` are printed by this repo's own tooling.
    'abct-503', 'at-503', 'st-503', 'rust-1.89', 'rust-1.89.0', 'gpt-5', 'gpt-4', 'opt-3',
    'apt-503', 'port-5030', 'commit-t-503', 'run-t-503', '2024-01-03t-503', '3t-503', '_t-503',
    'x_t-503', 'v1.t-503', 'Ωt-503', 'naïvet-503',
    // Path-shaped. Every one of these is something a build or an `ls` prints.
    '/tmp/t-503', './t-503', '~/t-503', 'src/t-503', '.cide/t-503', '.cide/worktrees/t-503',
    'feature/t-503', 't-503/', 't-503/src', 't-503/src/main.rs', 'https://example.com/t-503',
    'github.com/o/r/issues/t-503', 't-503.rs', 't-503.md', 't-503.json', 't-503.patch',
    // Welded on the right.
    't-503-fix', 't-503_x', 't-503abc', 't-503é',
    // Ten digits must be NOTHING, not a nine-digit prefix of somebody else's task.
    't-1234567890',
    // Malformed, and the long and upper-case spellings the matcher deliberately does not know.
    't-', 't--3', 't -503', 't- 503', 't_503', 't.503', 't503', '-503', 'T-503', 'T-5',
    'Task-503', 'task-503',
    // Not codes at all, from this repo's own output.
    '0.12.0', 'v0.1.0', '1.2s', '8ms', '#7', '--force-with-lease', '-j4', '', '   ',
    '  The task is already in doing. #7',
  ]) {
    eq(ids(line), [], `refuses ${JSON.stringify(line)}`)
  }

  // --- the line cap, where the maximality rule cannot hold -----------------------------------

  const inside = `${'x'.repeat(4000)} t-77 ${'y'.repeat(2000)}`
  eq(ids(inside), ['t-77'], 'a code inside the cap is found on an over-long line')
  eq(ids(`${'x'.repeat(4000)} ${'y'.repeat(2000)} t-77`), [],
    'and one past the cap is not scanned at all')
  const cut = `${'x'.repeat(MAX_LINE - 6)}t-5034${'z'.repeat(100)}`
  eq(ids(cut), [],
    'a match ending exactly at the cap is DROPPED: slicing t-5034 there mints t-503, a real and '
      + 'different task, and the lookahead cannot see what the slice removed')

  // --- the gate: no board, no link -----------------------------------------------------------

  const ready = (...list) => ({ kind: 'ready', tasks: list.map((id) => ({ id })) })
  const line = 't-1 t-2'
  const gate = (ctx) => offeredTaskCodes(line, ctx).map((c) => c.id)

  eq(gate({ paneProject: 'p', boardProject: 'p', board: ready('t-1', 't-2') }), ['t-1', 't-2'],
    'both codes are offered when the board holds both')
  eq(gate({ paneProject: 'p', boardProject: 'p', board: ready('t-1') }), ['t-1'],
    'only the id that is on the board — an unknown code stays plain text rather than '
      + 'underlining something that would then refuse')
  eq(gate({ paneProject: 'p', boardProject: 'p', board: ready('t-3', 't-4') }), [],
    'a board with neither offers nothing')
  eq(gate({ paneProject: 'p', boardProject: 'p', board: ready() }), [],
    'an empty board offers nothing')
  eq(gate({ paneProject: null, boardProject: 'p', board: ready('t-1') }), [],
    'a pane with no project offers nothing')
  eq(gate({ paneProject: '', boardProject: 'p', board: ready('t-1') }), [],
    "and `''` is the same answer as null — that is what TerminalPane pushes for such a pane")
  eq(gate({ paneProject: 'p', boardProject: null, board: ready('t-1') }), [],
    'a DETACHED-PANE WINDOW attaches the store to null and mounts no TaskDetailHost, so a code '
      + 'there gets no underline rather than one that swallows the click')
  eq(gate({ paneProject: 'a', boardProject: 'b', board: ready('t-1') }), [],
    'a pane whose project is not the board\'s offers nothing — every project can have a t-1')
  for (const kind of ['unknown', 'absent', 'unreadable']) {
    eq(gate({ paneProject: 'p', boardProject: 'p', board: { kind, tasks: [{ id: 't-1' }] } }), [],
      `a board that is ${kind} offers nothing, whatever it happens to carry`)
  }
  eq(gate({ paneProject: 'p', boardProject: 'p', board: { kind: READY } }), [],
    'a ready board with no tasks field at all offers nothing rather than throwing')
  for (const hostile of ['constructor', '__proto__', 'toString']) {
    eq(offeredTaskCodes(hostile, { paneProject: 'p', boardProject: 'p', board: ready('t-1') }), [],
      `a prototype key finds nothing (${hostile}) — the lookup is \`some\`, never an index`)
  }

  // --- the buffer walk and the offset arithmetic ----------------------------------------------

  const fake = (rows) => ({
    getLine: (y) =>
      y < 0 || y >= rows.length
        ? undefined
        : { isWrapped: rows[y].wrapped === true, translateToString: () => rows[y].text },
  })

  eq(logicalLine(fake([{ text: 'alone' }]), 0), { top: 0, rows: ['alone'] },
    'an unwrapped row is its own logical line')
  eq(logicalLine(fake([{ text: 'a' }]), 3), null, 'a row that does not exist is null, not empty')
  const wrapped = fake([
    { text: 'zero' },
    { text: 'one' },
    { text: 'two', wrapped: true },
    { text: 'three', wrapped: true },
    { text: 'four' },
  ])
  eq(logicalLine(wrapped, 2), { top: 1, rows: ['one', 'two', 'three'] },
    'hovered from the MIDDLE of a wrapped block, the walk goes up to the unwrapped row and down '
      + 'to the last wrapped one — a link on a wrapped line is the case a viewport-only read misses')
  eq(logicalLine(wrapped, 1).rows.length, 3, 'and from its top row it finds the same three')
  eq(logicalLine(wrapped, 4), { top: 4, rows: ['four'] }, 'the row after the block stands alone')

  const long = fake(
    Array.from({ length: 200 }, (_, i) => ({ text: `r${i}`, wrapped: i > 0 })),
  )
  ok(logicalLine(long, 150).rows.length <= MAX_SPAN_ROWS + 1,
    'the walk gives up rather than joining a pathological buffer inside one hover frame')

  const one = { top: 0, rows: ['x'] }
  eq(spanOf(one, 10, 0, 5), { start: { x: 1, y: 1 }, end: { x: 5, y: 1 } },
    'a span inside one row: 1-based, and `end` INCLUSIVE, which is what ILink.range wants')
  eq(spanOf({ top: 4, rows: ['x'] }, 10, 12, 16), { start: { x: 3, y: 6 }, end: { x: 6, y: 6 } },
    'an offset past the width lands on a later row, measured from the logical line\'s top')
  eq(spanOf(one, 10, 8, 12), { start: { x: 9, y: 1 }, end: { x: 2, y: 2 } },
    'a span crossing a row edge wraps, which is only correct because the rows are UNTRIMMED')

  // --- the one line that both matchers see ----------------------------------------------------

  const tool = '● cide_task_get  t-14  8ms #0'
  const run = parseRunLine(tool)
  const [inTitle] = matchTaskCodes(tool)
  eq(inTitle.id, 't-14', 'the code in a tool line\'s title is found')
  ok(inTitle.start >= run.toolEnd && inTitle.end <= run.tokenStart,
    'and it lies strictly between the run link\'s prefix and its handle token, so xterm\'s '
      + '_removeIntersectingLinks cannot drop one for the other — the run provider leaves the '
      + 'title alone precisely so this link can exist')

  // --- the wiring, all over stripped source ---------------------------------------------------

  const hosts = code(UI, 'src', 'layout', 'paneHosts.ts')
  ok(hosts.includes('attachTaskLinks('),
    'paneHosts.ts attaches the provider — this one line is the feature\'s only call site, and '
      + 'this project has shipped features reachable from nothing')

  /*
   * `openTerminal` and not `ensureTerminal`: the first builds the terminal, the second opens it
   * into the host element, and every listener and provider hangs off the second — which is also
   * where `host.cleanup` is filled, so it is the function whose order this pin is about.
   */
  const opens = hosts.slice(hosts.indexOf('export function openTerminal'))
  const next = opens.indexOf('export function', 1)
  const body = next < 0 ? opens : opens.slice(0, next)
  const order = ['attachPathLinks(', 'attachRunLinks(', 'attachTaskLinks('].map((name) =>
    body.indexOf(name),
  )
  ok(order.every((at) => at >= 0), 'all three providers are attached in openTerminal')
  ok(order[0] < order[1] && order[1] < order[2],
    'and IN THAT ORDER: xterm keeps the earlier provider\'s link wherever two overlap, so a '
      + 'project that really contains a file named t-503 keeps its path link, and a code in a '
      + 'tool line\'s title survives only because the run provider claims the glyph and the #7. '
      + 'Nothing pinned this order before M60 — swapping two lines here is silent')

  const taskProvider = code(UI, 'src', 'terminal', 'taskLinkProvider.ts')
  const runProvider = code(UI, 'src', 'terminal', 'runLinkProvider.ts')
  for (const [name, text] of [
    ['taskLinkProvider.ts', taskProvider],
    ['runLinkProvider.ts', runProvider],
  ]) {
    ok(text.includes("from './bufferSpan'"), `${name} shares the buffer walk`)
    ok(!/function\s+logicalLine\s*\(/.test(text) && !/function\s+rangeOf\s*\(/.test(text),
      `${name} keeps no private copy of the walk — the arithmetic lived inside a DOM callback `
        + 'until M60, which is the one place no check script can reach')
    const activate = text.slice(text.indexOf('activate'))
    for (const guard of ['event.button !== 0', 'ctrlKey', 'metaKey', 'getSelection()']) {
      ok(activate.includes(guard),
        `${name}'s activate guards ${guard}: xterm's Linkifier.ts:220 checks neither the button `
          + 'nor the selection, so a right-click\'s mouseup, a middle-click paste and a '
          + 'drag-to-copy all activate a hovered link')
    }
  }

  ok(taskProvider.includes('offeredTaskCodes(') && !taskProvider.includes('matchTaskCodes('),
    'the provider goes through the gate and never matches for itself — one road into the rule, '
      + 'or an underline could be drawn that nothing is allowed to act on')

  const reveal = code(UI, 'src', 'chrome', 'taskReveal.ts')
  eq((reveal.match(/notifyFailure\(/g) ?? []).length, 3,
    'revealTask makes three refusals: wrong project, unreadable board, task gone')
  ok(reveal.includes('state.select(task)'), 'and exactly one of its four ends opens the card')
  ok(reveal.includes('state.project !== project'),
    'the cross-project guard, which is what stops a link drawn in one project opening another '
      + 'project\'s task of the same id — a different task, correctly rendered, indistinguishable')

  const agents = code(UI, 'src', 'sidebar', 'AgentsPanel', 'AgentsPanelHost.tsx')
  for (const [name, text] of [
    ['taskLinkProvider.ts', taskProvider],
    ['AgentsPanelHost.tsx', agents],
  ]) {
    ok(text.includes('revealTask('),
      `${name} raises the gesture through the one producer`)
    ok(!text.includes('.select('),
      `${name} never selects a task itself — two roads to one gesture that disagree is the split `
        + 'cide_git::push already paid for')
  }
  ok(!agents.includes('notifyFailure(BOARD_NOT_READABLE'),
    'and the panel no longer carries its own copy of the refusals')

  const sentences = [
    'The task board cannot be read',
    'is not on the board any more',
    'belongs to a project that is not the one on screen',
  ]
  // `-co --exclude-standard`: tracked AND untracked-but-not-ignored. A plain `ls-files` misses
  // a file that has not been committed yet, which is every file on the branch that adds one.
  const files = execFileSync('git', ['ls-files', '-co', '--exclude-standard', 'ui/src'],
    { cwd: ROOT, encoding: 'utf8' })
    .split('\n')
    .filter((p) => p.endsWith('.ts') || p.endsWith('.tsx'))
  for (const sentence of sentences) {
    const holders = files.filter((p) => src(ROOT, p).includes(sentence))
    eq(holders, ['ui/src/chrome/taskReveal.ts'],
      `${JSON.stringify(sentence)} is written in exactly one file — counting FILES rather than `
        + 'occurrences is what catches a copy pasted back into a panel')
  }

  // --- the vocabulary across the import barriers -----------------------------------------------

  const model = code(UI, 'src', 'sidebar', 'TasksPanel', 'model.ts')
  ok(model.includes(`kind: '${READY}'`),
    'taskLinks.ts\'s READY is the kind model.ts calls ready — the two may not import one another, '
      + 'and a rename on either side is a link that silently stops being offered')

  const app = code(UI, 'src', 'App.tsx')
  ok(/attachTasks\(\s*boot\?\.role\.kind === 'detachedPane' \? null/.test(app),
    'a detached-pane window still attaches no board, which is what the gate\'s boardProject null '
      + 'arm is about — and mounting TaskDetailHost there would only be correct together with '
      + 'attaching the store, which is a different change')

  const pane = code(UI, 'src', 'panes', 'TerminalPane.tsx')
  const env = pane.slice(pane.indexOf('setTaskLinkEnv(paneId, {'))
  ok(env.slice(0, env.indexOf('})')).includes('get project()'),
    'the pane\'s project is a GETTER: the provider lives on the host and the effect runs once per '
      + 'pane, so a project switched under a parked pane must be visible to the next hover')
  ok(pane.indexOf('setTaskLinkEnv(paneId, {') < pane.indexOf('if (spec === null) return'),
    'and it is pushed before the spec early return — a pane that spawns no process still holds '
      + 'output, and its codes must stay live')
  ok(pane.includes('setTaskLinkEnv(paneId, null)'),
    'and the cleanup makes them inert, or a pane unmounted from a closing project keeps offering '
      + 'that project\'s codes')

  const paths = code(UI, 'src', 'terminal', 'pathLinks.ts')
  ok(paths.includes('matchTaskCodes(miss.text)'),
    'a ctrl+click on a task code is answered as a task code: pathMatch.ts\'s BODY includes `-`, '
      + 'so t-503 is a path candidate and "does not name a file" is true and useless next to an '
      + 'underline a plain click would have opened')

  // --- the prefix is the one Rust mints ---------------------------------------------------------

  const tasks = code(ROOT, 'crates', 'cide-tasks', 'src', 'lib.rs')
  ok(tasks.includes('TaskId(format!("t-{n}"))'),
    'Rust mints the prefix this matcher looks for. Stripping matters twice over here: the doc '
      + 'comments in that file spell t-17 and t-0007 in prose')
  ok(tasks.includes('is_ascii_alphanumeric'),
    'well_formed_id still accepts more than the matcher does, deliberately: a hand-written '
      + '`spike` is a legal id and is NOT linked, because looking every word of output up against '
      + 'the board would light up prose')

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('task links: ok (matcher + gate + buffer spans + wiring)')
} finally {
  rmSync(out, { recursive: true, force: true })
}
