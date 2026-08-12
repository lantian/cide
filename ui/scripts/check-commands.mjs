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
import { readFileSync } from 'node:fs'
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

/* --------------------------------------------------------------- the `when` vocabulary */

const CONTEXT_FLAGS = [
  ...rustBody(commandsRs, 'pub const CONTEXT_FLAGS: &[&str] = &[', 'CONTEXT_FLAGS')
    .replace('pub const CONTEXT_FLAGS: &[&str] = &[', '')
    .matchAll(/"([^"]+)"/g),
].map((m) => m[1])
ok(CONTEXT_FLAGS.length >= 10, `read ${CONTEXT_FLAGS.length} context flags — the regex still matches`)

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
  const bound = [
    ...rustBody(keymapRs, 'pub fn defaults() -> Vec<Binding> {', 'defaults()').matchAll(
      /\("([^"]+)",\s*"([^"]+)"\)/g,
    ),
  ].map((m) => ({ key: m[1], command: m[2] }))
  ok(bound.length >= 16, `read ${bound.length} default bindings — the regex still matches`)
  const unavailable = new Set(COMMANDS.filter((c) => c.unavailable !== null).map((c) => c.id))
  for (const { key, command } of bound) {
    ok(!unavailable.has(command), `${key} is not bound to the unavailable command ${command}`)
    ok(
      COMMANDS.some((c) => c.id === command),
      `${key} is bound to ${command}, which is a registered command`,
    )
  }
}

if (failed > 0) {
  console.error(`\ncheck-commands: ${failed} failure(s)`)
  process.exit(1)
}
console.log(
  `check-commands: ok (${COMMANDS.length} commands, ${HANDLERS.length} dispatched, ` +
    `${COMMANDS.filter((c) => c.unavailable !== null).length} unavailable with a reason)`,
)
