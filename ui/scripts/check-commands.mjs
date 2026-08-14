/**
 * **Every command in the registry is either handled or explained.** The gate that would have
 * caught this round's bug.
 *
 * `cide_core::commands` declared 42 commands. `ui/src/keys/dispatch.ts` had 7 `case` arms.
 * The keymap resolved a chord to a command id perfectly and handed it to a dispatcher with no
 * arm for it, so ~35 bound keystrokes were swallowed and did nothing — and the command palette
 * ran the same dispatcher over the whole registry, so it was *advertising* those 35 as rows
 * you could pick. Nothing failed. Every Rust test passed, the keymap tests passed, and
 * `check-key-gate.mjs` passed because it asserts that a chord *resolves*, not that anything is
 * listening at the other end.
 *
 * So this script compares the two lists directly:
 *
 *   * every id in `cide_core::commands::registry()` has a `case` in `dispatch.ts`, **or** an
 *     `unavailable` reason saying why it cannot work in this build;
 *   * no `case` names an id that is not registered (a typo in a case label is invisible
 *     otherwise — the arm simply never runs);
 *   * no id is both handled and unavailable, which would be the palette greying out a row
 *     that actually works;
 *   * no `case` body just calls `deps.fallback(`, which is a dead command wearing a switch
 *     arm — `file.save` shipped in exactly that shape once;
 *   * every context flag a `when` clause names is in `CONTEXT_FLAGS`, and every flag in
 *     `CONTEXT_FLAGS` is supplied by somebody. A clause naming a flag nobody sets is false
 *     for ever, so the command vanishes from the palette on every platform with nothing
 *     anywhere reporting it — which is precisely what had happened to `repoOpen`, and with it
 *     to all four git commands.
 *
 * Source text rather than an import: `dispatch.ts` pulls in zustand stores, the IPC client and
 * React-adjacent modules, so it cannot be compiled and loaded standalone the way
 * `check-key-gate.mjs` compiles the four pure key modules. The question being asked is
 * genuinely "is this id written down as a case", which is what a `switch` is, and
 * [`missing`] — the comparison itself — is pure and is self-tested below against a table with
 * a hole punched in it.
 *
 * Run: `pnpm --dir ui run check:commands`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}
const eq = (actual, expected, what) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(what, `actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
  }
}

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')

/** Line comments out, block comments out. Every scan below wants code, not prose. */
const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

/** The body of a Rust `fn name(...) { ... }`, found by its opening line and its `\n}`. */
function rustBody(source, opening, what) {
  const start = source.indexOf(opening)
  if (start < 0) throw new Error(`could not find ${what}`)
  const end = source.indexOf('\n}', start)
  return source.slice(start, end)
}

/* ------------------------------------------------------------------ the Rust registry */

const commandsRs = read('../../crates/cide-core/src/commands.rs')

/**
 * The shipped table, read out of the source that defines it.
 *
 * A regex rather than a parser, for the same reason `check-key-gate.mjs` reads
 * `keymap::defaults()` with one: the shape is a literal `vec![]` of builder chains in a single
 * function. The count assertion below is what stops a change of shape from silently matching
 * nothing and passing.
 */
function registry() {
  const body = stripComments(rustBody(commandsRs, 'fn build() -> Vec<Command> {', 'build()'))
  const chunks = body.split('Command::new(').slice(1)
  return chunks.map((chunk) => {
    const id = /^\s*"([^"]+)"/.exec(chunk)
    if (id === null) throw new Error(`a Command::new with no id literal: ${chunk.slice(0, 60)}`)
    const when = /\.when\(\s*"([^"]+)"/.exec(chunk)
    const unavailable = /\.unavailable\(\s*"([^"]+)"/.exec(chunk)
    return {
      id: id[1],
      when: when === null ? null : when[1],
      unavailable: unavailable === null ? null : unavailable[1],
    }
  })
}

const COMMANDS = registry()
ok(COMMANDS.length >= 35, `read ${COMMANDS.length} commands out of commands.rs — the regex still matches`)
ok(
  COMMANDS.some((c) => c.unavailable !== null),
  'at least one command carries an unavailable reason — the regex still matches',
)

/* --------------------------------------------------------------- the TypeScript switch */

const dispatchTs = read('../src/keys/dispatch.ts')

/**
 * Every id the dispatcher's switch names, and the body each arm falls into.
 *
 * Fall-through arms are real here (`pane.navigate.*` share one body, as do `tab.next` and
 * `tab.prev`), so a label with an empty body is not a defect — it is the next label's body
 * that has to be checked.
 */
function handlers() {
  const source = stripComments(dispatchTs)
  const marks = [...source.matchAll(/case '([a-zA-Z][\w.]*)':/g)]
  const end = source.indexOf('default:', marks.at(-1)?.index ?? 0)
  return marks.map((mark, i) => {
    const from = mark.index + mark[0].length
    const to = i + 1 < marks.length ? marks[i + 1].index : end < 0 ? source.length : end
    return { id: mark[1], body: source.slice(from, to) }
  })
}

const HANDLERS = handlers()
ok(HANDLERS.length >= 30, `found ${HANDLERS.length} dispatched commands — the scan still works`)

/* ------------------------------------------------------------------- the comparison */

/**
 * The heart of it, kept pure so it can be tested with a hole punched in its input.
 *
 * Returns the three ways the two lists can disagree. All three are silent in production:
 * an unhandled id is a keystroke that does nothing, an unknown handler is an arm that never
 * runs, and a handled-but-unavailable id is a greyed row for a command that works.
 */
export function missing(commands, handled) {
  const ids = new Set(commands.map((c) => c.id))
  const runnable = commands.filter((c) => c.unavailable === null).map((c) => c.id)
  const unavailable = new Set(commands.filter((c) => c.unavailable !== null).map((c) => c.id))
  const has = new Set(handled)
  return {
    unhandled: runnable.filter((id) => !has.has(id)),
    unknown: handled.filter((id) => !ids.has(id)),
    handledButUnavailable: handled.filter((id) => unavailable.has(id)),
  }
}

{
  const result = missing(COMMANDS, HANDLERS.map((h) => h.id))
  eq(
    result.unhandled,
    [],
    'every runnable command has a case in dispatch.ts (an unhandled id is a key that is ' +
      'swallowed and a palette row that does nothing — mark it `.unavailable("…")` in ' +
      'commands.rs if it genuinely cannot work yet)',
  )
  eq(result.unknown, [], 'every case in dispatch.ts names a registered command')
  eq(result.handledButUnavailable, [], 'no command is both dispatched and marked unavailable')
}

{
  /*
   * The check's own test. A gate nobody has seen fail is a gate nobody knows works — and
   * this one is asserting the *absence* of something, which is the shape that passes
   * vacuously when its scan breaks.
   */
  const fixture = [
    { id: 'tab.close', when: null, unavailable: null },
    { id: 'tab.next', when: null, unavailable: null },
    { id: 'pane.promoteToTab', when: null, unavailable: 'needs a domain command' },
  ]
  eq(
    missing(fixture, ['tab.next']).unhandled,
    ['tab.close'],
    'the check reports an unhandled id (self-test)',
  )
  eq(
    missing(fixture, ['tab.close', 'tab.next', 'tab.closeAllOthers']).unknown,
    ['tab.closeAllOthers'],
    'the check reports a case for an unregistered command (self-test)',
  )
  eq(
    missing(fixture, ['tab.close', 'tab.next', 'pane.promoteToTab']).handledButUnavailable,
    ['pane.promoteToTab'],
    'the check reports a command that is both handled and unavailable (self-test)',
  )
  eq(
    missing(fixture, ['tab.close', 'tab.next']),
    { unhandled: [], unknown: [], handledButUnavailable: [] },
    'the check passes a table with no holes in it (self-test)',
  )
}

{
  /*
   * A `case` that forwards to `fallback` is not a dispatcher.
   *
   * `file.save` shipped for review in exactly that shape: a case existed, so a scan for case
   * labels was satisfied, and the body's first branch handed the command straight back to
   * `deps.fallback` because the host had not supplied a `focusedTab`. Ctrl+S still did
   * nothing; the only change was that the log line came from a different switch arm.
   */
  for (const { id, body } of HANDLERS) {
    ok(
      !body.includes('deps.fallback('),
      `dispatch.ts handles ${id} rather than forwarding it to fallback`,
    )
  }
}

{
  /*
   * ...and a `case` whose call answers "nothing to do" and is not read is not a dispatcher
   * either. Same defect, one layer down.
   *
   * `file.reveal` was the live example. It opened the Files panel and called
   * `treeStore.reveal(path)`, which asks the index-only `fs_reveal`; that answers `null` for
   * a gitignored file, for one deleted between the pick and the reveal, and — now that a file
   * outside the project can be opened at all — for the ordinary case of an out-of-project tab.
   * The store swallowed all three and the promise was `void`ed, so the command was listed,
   * enabled, bindable, and silently did nothing.
   */
  const reveal = HANDLERS.find((h) => h.id === 'file.reveal')
  ok(reveal !== undefined, 'file.reveal still has a case in dispatch.ts')
  if (reveal) {
    ok(
      /\.then\(/.test(reveal.body) && /notify\(/.test(reveal.body),
      'file.reveal reads whether the reveal landed and says so when it did not — a command ' +
        'that opens a panel and then does nothing is indistinguishable from one wired to ' +
        'nothing, which is the defect this project has now found more than a dozen times',
    )
  }
}

{
  /*
   * ...and a `case` that shows the user the thing they asked for but leaves the keyboard
   * somewhere else is three quarters of a dispatcher. Same defect again, one layer further out.
   *
   * `tab.console` is Ctrl+1 — *go to the Claude console* — and `ws.activateTab` alone makes the
   * tab visible while the caret stays in whatever the user was typing in, which for a command
   * whose entire purpose is "put me in the prompt" is the half that matters. `revealPane` is the
   * one function that activates the project, activates the tab, drops a maximize that would hide
   * the pane, moves the domain's focus and *then* focuses the terminal, in the order that works.
   * The same rule the tab switcher's commit follows, for the same reason.
   */
  const console_ = HANDLERS.find((h) => h.id === 'tab.console')
  ok(console_ !== undefined, 'tab.console still has a case in dispatch.ts')
  if (console_) {
    ok(
      /revealPane\(/.test(console_.body),
      'tab.console goes through revealPane rather than activating a tab and stopping there — a ' +
        'console the user is looking at but cannot type into is half the command',
    )
    ok(
      /consolePaneOf\(/.test(console_.body),
      'and names the console by identity (`tabs[0]`) rather than reusing the mention target, ' +
        'which prefers whichever Claude pane happens to have focus',
    )
  }
}

/* --------------------------------------------------------------- the `when` vocabulary */

/**
 * The vocabulary, read out of the `const` that declares it.
 *
 * Bounded by the array's own `];` and stripped of comments before the string literals are
 * collected. Both matter and neither used to be true: the reader ran to the next `\n}`, which
 * is the end of the *function after* the constant, and it kept every quoted word it passed on
 * the way — so a `//` comment inside the array naming a flag, or merely quoting a phrase,
 * became a phantom entry in the vocabulary and failed the "supplied by somebody" check below
 * with a sentence in place of a flag name. A comment explaining why a flag was removed is
 * exactly the comment that belongs in that array.
 */
const CONTEXT_FLAGS = (() => {
  const opening = 'pub const CONTEXT_FLAGS: &[&str] = &['
  const start = commandsRs.indexOf(opening)
  if (start < 0) throw new Error('could not find CONTEXT_FLAGS in commands.rs')
  const end = commandsRs.indexOf('];', start)
  if (end < 0) throw new Error('CONTEXT_FLAGS has no closing `];`')
  const body = stripComments(commandsRs.slice(start + opening.length, end))
  return [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1])
})()
ok(CONTEXT_FLAGS.length >= 10, `read ${CONTEXT_FLAGS.length} context flags — the regex still matches`)
ok(
  !CONTEXT_FLAGS.some((flag) => flag.includes(' ')),
  `every flag read out of CONTEXT_FLAGS is an identifier, not prose: ${JSON.stringify(CONTEXT_FLAGS)}`,
)

const contextTs = read('../src/keys/context.ts')
const flagList = (name) => {
  const found = new RegExp(`export const ${name} = \\[([^\\]]*)\\]`).exec(contextTs)
  if (found === null) throw new Error(`could not find ${name} in keys/context.ts`)
  return [...found[1].matchAll(/'([^']+)'/g)].map((m) => m[1])
}
const DERIVED_FLAGS = flagList('DERIVED_FLAGS')
const HOST_FLAGS = flagList('HOST_FLAGS')

{
  // Every flag the vocabulary allows must be set by somebody, or a clause naming it is false
  // for ever and the command it guards is unreachable in silence.
  const supplied = new Set([...DERIVED_FLAGS, ...HOST_FLAGS])
  eq(
    CONTEXT_FLAGS.filter((flag) => !supplied.has(flag)),
    [],
    'every flag in CONTEXT_FLAGS is derived in keys/context.ts or listed as a host flag',
  )
  // And the other way: a derived flag no clause may name is dead code in the hot path.
  const allowed = new Set(CONTEXT_FLAGS)
  eq(
    [...DERIVED_FLAGS, ...HOST_FLAGS].filter((flag) => !allowed.has(flag)),
    [],
    'every flag keys/context.ts supplies is in the Rust CONTEXT_FLAGS vocabulary',
  )

  // `deriveContext` must actually return each derived flag, not merely list its name.
  const derive = contextTs.slice(contextTs.indexOf('export function deriveContext'))
  for (const flag of DERIVED_FLAGS) {
    ok(new RegExp(`\\b${flag}:`).test(derive), `deriveContext returns ${flag}`)
  }

  // The host flags are `App.tsx`'s to supply. It is not this milestone's file, so this is
  // the only way to notice one of them going missing.
  const appTsx = read('../src/App.tsx')
  for (const flag of HOST_FLAGS) {
    ok(new RegExp(`\\b${flag}\\b`).test(appTsx), `App.tsx still supplies the host flag ${flag}`)
  }
}

{
  // Clause identifiers, checked here as well as in Rust so the message arrives with the rest.
  for (const { id, when } of COMMANDS) {
    if (when === null) continue
    for (const flag of when.split(/[^A-Za-z0-9_.]+/).filter(Boolean)) {
      ok(CONTEXT_FLAGS.includes(flag), `${id}'s when clause names the known flag ${flag}`)
    }
  }
}

/* ------------------------------------------------------- nothing binds a dead command */

{
  // Also asserted in Rust. Repeated here because this is the script a frontend change runs,
  // and a key bound to an unavailable command is a *swallowed* keystroke: the gate consumes
  // it, so the editor and the pty never see it either.
  const keymapRs = read('../../crates/cide-core/src/keymap.rs')
  const defaultsBody = rustBody(keymapRs, 'pub fn defaults() -> Vec<Binding> {', 'defaults()')
  const bound = [
    ...defaultsBody.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g),
  ].map((m) => ({ key: m[1], command: m[2] }))
  /*
   * The `when`-carrying entries, which are 3-tuples and which the 2-tuple regex above cannot
   * see at all. M12 added the first of them (`alt+up` / `alt+down`, scoped to `editorFocused`),
   * and without this they would escape *both* gates below in silence — a key bound to an
   * unavailable command, or to no command at all, is exactly what this block exists to catch,
   * and "the regex did not match it" is the quietest possible way to fail.
   */
  bound.push(
    ...[...defaultsBody.matchAll(/\("([^"]+)",\s*"([^"]+)",\s*"([^"]+)"\)/g)].map((m) => ({
      key: m[1],
      command: m[2],
      when: m[3],
    })),
  )
  ok(bound.length >= 20, `read ${bound.length} default bindings — the regex still matches`)
  ok(
    bound.some((b) => b.when !== undefined),
    'no `when`-carrying default binding was found — the 3-tuple regex has stopped matching',
  )
  const unavailable = new Set(COMMANDS.filter((c) => c.unavailable !== null).map((c) => c.id))
  for (const { key, command } of bound) {
    ok(!unavailable.has(command), `${key} is not bound to the unavailable command ${command}`)
    ok(
      COMMANDS.some((c) => c.id === command),
      `${key} is bound to ${command}, which is a registered command`,
    )
  }
}

/* ------------------------------------------------- what "focused" means, per window role */

/*
 * `keys/target.ts` decides the project, the tab and the pane every handler acts on, and
 * every `when` flag `keys/context.ts` derives. It is pure and its only import is a `type`
 * one, so unlike `dispatch.ts` it can be compiled and driven from fixtures — and it has to
 * be, because the three window roles are the case source review cannot check by reading.
 *
 * The bug this pins: a `detachedPane` role carries `tab`, naming the tab the pane was torn
 * *out* of — which is still in the shell window. Reading it made `focusTarget` resolve, in
 * the detached window, to whatever pane the shell window had focused, and `App.tsx` installs
 * the key gate before it branches on the role. Ctrl+W in a detached pane closed a tab in
 * another window. Nothing failed: the id was registered, the case existed, the chord
 * resolved, and `check-commands` itself was green.
 */
{
  const uiDir = fileURLToPath(new URL('..', import.meta.url))
  const out = mkdtempSync(join(tmpdir(), 'cide-commands-'))
  const config = join(out, 'tsconfig.json')
  // Written next to the output rather than committed: `baseUrl` is resolved from the
  // config's own directory, so an absolute one lets the `@/*` alias work from anywhere.
  writeFileSync(
    config,
    JSON.stringify({
      compilerOptions: {
        module: 'commonjs',
        moduleResolution: 'node10',
        target: 'es2022',
        strict: true,
        exactOptionalPropertyTypes: true,
        noUncheckedIndexedAccess: true,
        baseUrl: uiDir,
        paths: { '@/*': ['src/*'] },
        rootDir: join(uiDir, 'src'),
        outDir: join(out, 'js'),
        // Types only, and the DTOs drag in the whole IPC client. Emitting the one file and
        // skipping the library check keeps this to the module under test.
        skipLibCheck: true,
        types: [],
      },
      files: [join(uiDir, 'src/keys/target.ts')],
    }),
  )
  execFileSync('node', ['node_modules/typescript/bin/tsc', '--project', config], {
    cwd: uiDir,
    stdio: 'inherit',
  })

  const target = createRequire(import.meta.url)(join(out, 'js/keys/target.js'))

  const pane = (id, kind) => ({ id, kind, session: null })
  const tab = (id, panes, focused) => ({
    id,
    kind: { kind: 'claude' },
    tree: { root: null, focused, maximized: null, panes: Object.fromEntries(panes.map((p) => [p.id, p])) },
  })
  const root = (path, label) => ({ path, label })
  /** One workspace, three windows onto it. `t1` is the pinned console of `p1`. */
  const boot = (role) => ({
    role,
    commands: [],
    workspace: {
      rev: 1,
      settings: { windowMode: 'stacked' },
      windows: {},
      projects: {
        p1: {
          id: 'p1',
          name: 'one',
          activeTab: 't1',
          roots: [root('/a', 'a')],
          detached: { pD: pane('pD', 'shell') },
          tabs: [tab('t1', [pane('pA', 'claude')], 'pA'), tab('t2', [pane('pB', 'editor')], 'pB')],
        },
        p2: {
          id: 'p2',
          name: 'two',
          activeTab: 't3',
          roots: [root('/b', 'b')],
          detached: {},
          tabs: [tab('t3', [pane('pC', 'claude')], 'pC')],
        },
      },
    },
  })

  /*
   * The fixture is hand-written, so check its shape against the DTO Rust actually produces.
   *
   * **This is the assertion that was missing.** Every root above used to carry a `repo`, and
   * `keys/target.ts::reposOf` read it to derive the `repoOpen` context flag that gated every
   * git command. Rust has never put anything but `None` in that field — the fixture was
   * describing a `ProjectRoot` no build has ever produced — so the flag was false for every
   * user, the whole Git group vanished from the palette, and this script stayed green because
   * every check it ran was fed the same invented shape. A hand-written fixture is a claim
   * about the wire, and an unchecked claim about the wire is how a permanently-false flag
   * looks exactly like a working one.
   *
   * Both directions are asserted. A field the DTO does not have is the bug above; a field it
   * has and the fixture omits is a fact `target.ts` could start reading tomorrow with nothing
   * here exercising it.
   */
  {
    const generated = read('../src/ipc/generated.ts')
    const at = generated.indexOf('export type ProjectRoot = {')
    if (at < 0) throw new Error('could not find ProjectRoot in ipc/generated.ts')
    const end = generated.indexOf('};', at)
    const declared = [
      ...stripComments(generated.slice(at, end)).matchAll(/[{,]\s*([A-Za-z_]\w*)\??:/g),
    ].map((m) => m[1])
    ok(declared.length >= 2, `read ${declared.length} ProjectRoot fields — the scan still works`)
    const fixture = Object.keys(root('/a', 'a'))
    eq(
      [...fixture].sort(),
      [...declared].sort(),
      'the bootstrap fixture builds a real ProjectRoot — every field the generated DTO ' +
        'declares and no field it does not (a fixture carrying a field Rust never fills is ' +
        'what let `repoOpen` be permanently false with this script green)',
    )
  }

  const shell = boot({ kind: 'shell', projects: ['p1', 'p2'], active: 'p1' })
  const detachedPane = boot({ kind: 'detachedPane', project: 'p1', tab: 't1', pane: 'pD' })
  const detachedTab = boot({ kind: 'detachedTab', project: 'p1', tab: 't2' })

  eq(
    target.focusTarget(shell)?.pane.id,
    'pA',
    'the shell window focuses its active tab’s focused pane',
  )
  eq(
    target.focusTarget(detachedPane),
    null,
    'a detached-pane window has no focus target (its role.tab names a tab in the shell ' +
      'window, so anything else makes every pane and tab command act on the wrong window)',
  )
  eq(target.activeTabOf(detachedPane), null, 'a detached-pane window shows no tab')
  eq(
    target.focusTarget(detachedTab)?.tab.id,
    't2',
    'a detached-tab window focuses the tab it was opened for',
  )

  // `closableTab`, which `tab.close` re-checks: `tabs[0]` is the pinned console and
  // `close_tab` answers `TabPinned`, so Ctrl+W there must report rather than raise a
  // "close anyway?" dialog about a tab that cannot close.
  eq(target.isClosableTab(shell), false, 'the pinned project console is not closable')
  const onSecondTab = structuredClone(shell)
  onSecondTab.workspace.projects.p1.activeTab = 't2'
  eq(target.isClosableTab(onSecondTab), true, 'an ordinary tab is closable')
  eq(target.isClosableTab(detachedPane), false, 'a detached-pane window closes no tab')

  // The project strip Ctrl+Tab cycles is the *window's*, not the workspace's. In
  // `perProject` mode the workspace holds every project and each window holds one, and
  // `activate_project` only touches windows whose strip contains the id — so cycling the
  // workspace list there is a keystroke that does nothing.
  eq([...target.windowProjectsOf(shell)], ['p1', 'p2'], 'the shell window cycles its own strip')
  eq(
    [...target.windowProjectsOf(boot({ kind: 'shell', projects: ['p1'], active: 'p1' }))],
    ['p1'],
    'a per-project window holds one project however many the workspace has',
  )
  eq([...target.windowProjectsOf(detachedTab)], [], 'a detached window has no project strip')
}

if (failed > 0) {
  console.error(`\ncheck-commands: ${failed} failure(s)`)
  process.exit(1)
}
console.log(
  `check-commands: ok (${COMMANDS.length} commands, ${HANDLERS.length} dispatched, ` +
    `${COMMANDS.filter((c) => c.unavailable !== null).length} unavailable with a reason)`,
)
