/**
 * The key gate's unit test: **both entry points resolve every chord identically**.
 *
 * Without this, someone adds a binding to one path and Ctrl+P works everywhere except
 * terminals — a failure with no error and no symptom until a user types `^P` into a shell
 * and gets a shell command instead of the file picker.
 *
 * The corpus is not the shipped keymap. It is the *whole* chord space that matters: every
 * one of the sixteen modifier combinations against a spread of key names, plus every
 * multi-stroke sequence in the fixture. Testing only the bindings that exist today would
 * pass on a keymap whose resolution is wrong for everything else, and would need editing
 * every time `cide-core::keymap::defaults` gains a line.
 *
 * This project has no JS test runner and adding one for four pure modules would be a larger
 * commitment than the code it tests, so this follows `check-status-format.mjs`: compile the
 * modules with the TypeScript that is already in `node_modules`, then exercise the output.
 * CommonJS emit rather than ESM, because `import './chords'` without a file extension —
 * which is what `moduleResolution: bundler` lets the app source write — does not resolve
 * under Node's ESM loader, whereas `require('./chords')` does.
 *
 * Run: `pnpm --dir ui run check:keys`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * The `(key, command)` pairs of `cide_core::keymap::defaults()`, read out of the Rust source.
 *
 * The fixture below mirrors that table by hand, and the specific claims this script makes —
 * `ctrl+p` is `picker.files`, `ctrl+shift+\`` is `terminal.splitBelow` — are only worth
 * anything if the mirror is current. Reading the Rust is the difference between a rename over
 * there failing here and a rename over there leaving a green suite asserting a binding that no
 * longer exists.
 *
 * A regex rather than a parser because the shape being read is a literal array of string
 * tuples in one function; if that ever stops being true, the count assertion below fails
 * loudly rather than silently matching nothing.
 */
function rustDefaults() {
  const path = fileURLToPath(new URL('../../crates/cide-core/src/keymap.rs', import.meta.url))
  const source = readFileSync(path, 'utf8')
  const start = source.indexOf('pub fn defaults() -> Vec<Binding> {')
  if (start < 0) throw new Error(`could not find defaults() in ${path}`)
  const end = source.indexOf('\n}', start)
  const body = source.slice(start, end)
  return [...body.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => ({ key: m[1], command: m[2] }))
}

const out = mkdtempSync(join(tmpdir(), 'cide-keys-'))
let failed = 0

const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  if (!Object.is(actual, expected)) {
    fail(what, `actual:   ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
  }
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/keys/chords.ts',
      'src/keys/when.ts',
      'src/keys/keymap.ts',
      'src/keys/gate.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      // The gate names `window` and `setTimeout` behind runtime guards, so it needs the DOM
      // declarations to typecheck even though nothing here touches a DOM.
      '--lib', 'es2023,dom',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const { createKeyGate } = require(join(out, 'keys/gate.js'))
  const { buildKeymap } = require(join(out, 'keys/keymap.js'))
  const { chipLabel, normalizeSequence, strokeFromEvent } = require(join(out, 'keys/chords.js'))
  const { evaluateWhen } = require(join(out, 'keys/when.js'))

  /* ------------------------------------------------------------------ the fixture keymap */

  /*
   * Mirrors `cide_core::keymap::defaults()` plus two things the shipped table does not have
   * yet and the gate must handle anyway: a multi-stroke sequence, and a `when`-gated
   * binding that shadows a terminal key.
   */
  const BINDINGS = [
    { key: 'ctrl+alt+right', command: 'pane.split.right', when: null },
    { key: 'ctrl+alt+down', command: 'pane.split.down', when: null },
    { key: 'ctrl+shift+n', command: 'claude.split.newSession', when: null },
    { key: 'ctrl+shift+up', command: 'pane.promoteToTab', when: null },
    { key: 'ctrl+shift+d', command: 'pane.detachToWindow', when: null },
    { key: 'ctrl+shift+`', command: 'terminal.splitBelow', when: null },
    { key: 'ctrl+p', command: 'picker.files', when: null },
    { key: 'ctrl+shift+p', command: 'palette.commands', when: null },
    { key: 'ctrl+w', command: 'tab.close', when: null },
    { key: 'ctrl+s', command: 'file.save', when: null },
    { key: 'ctrl+shift+t', command: 'theme.toggle', when: null },
    { key: 'ctrl+alt+h', command: 'pane.navigate.left', when: null },
    { key: 'ctrl+alt+l', command: 'pane.navigate.right', when: null },
    { key: 'ctrl+alt+k', command: 'pane.navigate.up', when: null },
    { key: 'ctrl+alt+j', command: 'pane.navigate.down', when: null },
    { key: 'ctrl+comma', command: 'settings.open', when: null },
    { key: 'ctrl+k ctrl+s', command: 'settings.keymap', when: null },
    { key: 'ctrl+k ctrl+w', command: 'tab.closeOthers', when: null },
    { key: 'ctrl+shift+c', command: 'terminal.clear', when: 'terminalFocused' },
    { key: 'ctrl+e', command: 'file.reveal', when: 'editorFocused && !overlayOpen' },
  ]

  /* The fixture is a mirror, so check it against the thing it mirrors before trusting it. */
  {
    const rust = rustDefaults()
    ok(rust.length >= 16, `read ${rust.length} defaults out of keymap.rs — the regex still matches`)
    for (const { key, command } of rust) {
      const mirrored = BINDINGS.some((b) => b.key === key && b.command === command)
      ok(mirrored, `the fixture mirrors cide_core::keymap::defaults(): ${key} → ${command}`)
    }
  }

  /** Physical keys worth sweeping every modifier combination against. */
  const KEYS = [
    { code: 'KeyP', key: 'p' },
    { code: 'KeyS', key: 's' },
    { code: 'KeyW', key: 'w' },
    { code: 'KeyK', key: 'k' },
    { code: 'KeyN', key: 'n' },
    { code: 'KeyD', key: 'd' },
    { code: 'KeyT', key: 't' },
    { code: 'KeyC', key: 'c' },
    { code: 'KeyE', key: 'e' },
    { code: 'KeyH', key: 'h' },
    { code: 'KeyJ', key: 'j' },
    { code: 'KeyL', key: 'l' },
    { code: 'KeyA', key: 'a' },
    { code: 'Digit1', key: '1' },
    { code: 'ArrowUp', key: 'ArrowUp' },
    { code: 'ArrowDown', key: 'ArrowDown' },
    { code: 'ArrowLeft', key: 'ArrowLeft' },
    { code: 'ArrowRight', key: 'ArrowRight' },
    { code: 'Comma', key: ',' },
    // Shift on this key produces `~`, which is exactly why the gate reads `code`.
    { code: 'Backquote', key: '`' },
    { code: 'Enter', key: 'Enter' },
    { code: 'Escape', key: 'Escape' },
    { code: 'Tab', key: 'Tab' },
    { code: 'F1', key: 'F1' },
    { code: 'Space', key: ' ' },
  ]

  const CONTEXTS = [
    {},
    { terminalFocused: true },
    { editorFocused: true },
    { editorFocused: true, overlayOpen: true },
    { claudePaneFocused: true, repoOpen: true },
  ]

  const event = (spec) => ({
    type: 'keydown',
    code: spec.code,
    key: spec.key,
    ctrlKey: spec.ctrl === true,
    altKey: spec.alt === true,
    shiftKey: spec.shift === true,
    metaKey: spec.meta === true,
    prevented: 0,
    stopped: 0,
    preventDefault() {
      this.prevented += 1
    },
    stopPropagation() {
      this.stopped += 1
    },
  })

  /** A gate with a frozen clock, plus the log of what it dispatched. */
  const makeGate = (ctx) => {
    const dispatched = []
    let clock = 0
    const gate = createKeyGate({
      bindings: () => BINDINGS,
      context: () => ctx,
      run: (command, args) => dispatched.push({ command, args }),
      now: () => clock,
      timeoutMs: 1000,
    })
    return {
      gate,
      dispatched,
      advance: (ms) => {
        clock += ms
      },
    }
  }

  /* --------------------------------------------- the claim: both entry points agree */

  let compared = 0
  for (const ctx of CONTEXTS) {
    for (const spec of KEYS) {
      for (let bits = 0; bits < 16; bits++) {
        const mods = {
          ctrl: (bits & 1) !== 0,
          alt: (bits & 2) !== 0,
          shift: (bits & 4) !== 0,
          meta: (bits & 8) !== 0,
        }
        const label = `${JSON.stringify(mods)} ${spec.code} in ${JSON.stringify(ctx)}`

        // Two independent gates so the prefix machine of one cannot colour the other, and
        // two distinct event objects so the decision memo cannot hand the second entry
        // point the first one's answer.
        const a = makeGate(ctx)
        const b = makeGate(ctx)
        const evA = event({ ...spec, ...mods })
        const evB = event({ ...spec, ...mods })

        const viaWindow = a.gate.windowHandler(evA)
        const viaTerminal = b.gate.terminalHandler(evB)

        compared += 1
        if (viaWindow !== viaTerminal) {
          fail(`entry points disagree on pass-through: ${label}`, `window=${viaWindow} terminal=${viaTerminal}`)
        }
        if (JSON.stringify(a.dispatched) !== JSON.stringify(b.dispatched)) {
          fail(
            `entry points dispatched differently: ${label}`,
            `window=${JSON.stringify(a.dispatched)} terminal=${JSON.stringify(b.dispatched)}`,
          )
        }
        if (a.gate.pending() !== b.gate.pending()) {
          fail(`entry points left different prefixes armed: ${label}`)
        }
        // A swallowed chord must never reach the browser's own default on either path.
        if (!viaWindow && evA.prevented === 0) fail(`window entry did not preventDefault: ${label}`)
        if (!viaTerminal && evB.prevented === 0) {
          fail(`terminal entry did not preventDefault: ${label}`)
        }
        // Only the window entry stops the event reaching its target; the terminal entry is
        // consulted after the event has already arrived, and calling it there would be a
        // lie about what the handler can do.
        if (evB.stopped !== 0) fail(`terminal entry called stopPropagation: ${label}`)
      }
    }
  }
  ok(compared === CONTEXTS.length * KEYS.length * 16, 'swept the whole modifier × key space')

  /* Multi-stroke sequences, both entry points, stroke by stroke. */
  for (const sequence of [
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyS', key: 's', ctrl: true }],
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyW', key: 'w', ctrl: true }],
    // Second stroke completes nothing: swallowed, prefix disarmed.
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyZ', key: 'z', ctrl: true }],
    // Second stroke is an unmodified letter — must not reach the PTY either.
    [{ code: 'KeyK', key: 'k', ctrl: true }, { code: 'KeyA', key: 'a' }],
  ]) {
    const label = sequence.map((s) => s.code).join(' ')
    const a = makeGate({})
    const b = makeGate({})
    const results = { window: [], terminal: [] }
    for (const spec of sequence) {
      results.window.push(a.gate.windowHandler(event(spec)))
      results.terminal.push(b.gate.terminalHandler(event(spec)))
    }
    eq(results.window.join(','), results.terminal.join(','), `sequence pass-through: ${label}`)
    eq(
      JSON.stringify(a.dispatched),
      JSON.stringify(b.dispatched),
      `sequence dispatch: ${label}`,
    )
    ok(results.window.every((through) => through === false), `sequence fully swallowed: ${label}`)
    eq(a.gate.pending(), null, `sequence disarmed the prefix: ${label}`)
  }

  /* ------------------------------------------------------------------ specific claims */

  // The M8 acceptance criterion: Ctrl+P with a terminal focused opens the picker and `^P`
  // never reaches the shell.
  {
    const t = makeGate({ terminalFocused: true })
    const through = t.gate.terminalHandler(event({ code: 'KeyP', key: 'p', ctrl: true }))
    eq(through, false, 'ctrl+p is swallowed before the PTY')
    eq(t.dispatched.length, 1, 'ctrl+p dispatched exactly one command')
    eq(t.dispatched[0]?.command, 'picker.files', 'ctrl+p ran picker.files')
  }

  // One event object seen by both entry points dispatches once, not twice.
  {
    const g = makeGate({})
    const ev = event({ code: 'KeyP', key: 'p', ctrl: true })
    const first = g.gate.windowHandler(ev)
    const second = g.gate.terminalHandler(ev)
    eq(first, second, 'a shared event decides the same way twice')
    eq(g.dispatched.length, 1, 'a shared event dispatches exactly once')
  }

  // keyup and keypress are not chords. xterm consults the custom handler for all three.
  {
    const g = makeGate({})
    for (const type of ['keyup', 'keypress']) {
      const ev = { ...event({ code: 'KeyP', key: 'p', ctrl: true }), type }
      eq(g.gate.terminalHandler(ev), true, `${type} passes through`)
    }
    eq(g.dispatched.length, 0, 'only keydown dispatches')
  }

  // A bare modifier is not a chord.
  {
    const g = makeGate({})
    eq(
      g.gate.windowHandler(event({ code: 'ControlLeft', key: 'Control', ctrl: true })),
      true,
      'a held Control passes through',
    )
    eq(g.gate.pending(), null, 'a held Control arms nothing')
  }

  // The prefix expires. A stale `ctrl+k` must not eat the next real keystroke.
  {
    const g = makeGate({})
    eq(g.gate.windowHandler(event({ code: 'KeyK', key: 'k', ctrl: true })), false, 'ctrl+k arms')
    eq(g.gate.pending(), 'ctrl+k', 'ctrl+k is the pending prefix')
    g.advance(1000)
    eq(
      g.gate.windowHandler(event({ code: 'KeyA', key: 'a' })),
      true,
      'a plain letter after the timeout passes through',
    )
    eq(g.gate.pending(), null, 'the stale prefix was dropped')
    eq(g.dispatched.length, 0, 'nothing ran')
  }

  // `when` decides, on both paths, and unknown flags read false.
  {
    const withTerm = makeGate({ terminalFocused: true })
    const without = makeGate({})
    const spec = { code: 'KeyC', key: 'c', ctrl: true, shift: true }
    eq(withTerm.gate.windowHandler(event(spec)), false, 'terminal.clear binds when a terminal has focus')
    eq(withTerm.dispatched[0]?.command, 'terminal.clear', 'terminal.clear ran')
    eq(without.gate.windowHandler(event(spec)), true, 'and passes through otherwise')
    eq(without.dispatched.length, 0, 'nothing ran without the context')
  }
  {
    const editor = makeGate({ editorFocused: true })
    const overlay = makeGate({ editorFocused: true, overlayOpen: true })
    const spec = { code: 'KeyE', key: 'e', ctrl: true }
    eq(editor.gate.windowHandler(event(spec)), false, '`editorFocused && !overlayOpen` holds')
    eq(overlay.gate.windowHandler(event(spec)), true, 'and is false with the overlay open')
  }

  // Shift + backquote reports `~` as `key`. Reading `code` is what makes the binding work.
  {
    const g = makeGate({})
    const through = g.gate.terminalHandler(
      event({ code: 'Backquote', key: '~', ctrl: true, shift: true }),
    )
    eq(through, false, 'ctrl+shift+` resolves despite `key` reporting ~')
    eq(g.dispatched[0]?.command, 'terminal.splitBelow', 'ctrl+shift+` ran terminal.splitBelow')
  }

  /* ------------------------------------------------------------ spelling and normalisation */

  eq(strokeFromEvent(event({ code: 'KeyP', key: 'p', ctrl: true })), 'ctrl+p', 'stroke spelling')
  eq(
    strokeFromEvent(event({ code: 'KeyP', key: 'P', ctrl: true, shift: true, alt: true, meta: true })),
    'ctrl+alt+shift+meta+p',
    'modifier order matches cide-core::keymap',
  )
  eq(strokeFromEvent(event({ code: 'Comma', key: ',', ctrl: true })), 'ctrl+comma', 'comma is named')
  eq(strokeFromEvent(event({ code: 'ArrowRight', key: 'ArrowRight' })), 'right', 'arrows are named')

  eq(normalizeSequence('Shift+Ctrl+P'), 'ctrl+shift+p', 'normalisation orders modifiers')
  eq(normalizeSequence('ctrl+,'), 'ctrl+comma', 'a written comma folds onto the named key')
  eq(normalizeSequence('CTRL+K   ctrl+S'), 'ctrl+k ctrl+s', 'sequences collapse whitespace')
  eq(normalizeSequence('ctrl+`'), 'ctrl+backquote', 'a written backtick folds onto the named key')

  eq(chipLabel('ctrl+shift+p'), '⌃⇧P', 'chip for a letter chord')
  eq(chipLabel('ctrl+alt+right'), '⌃⌥→', 'chip for an arrow chord')
  eq(chipLabel('ctrl+k ctrl+s'), '⌃K ⌃S', 'chip for a sequence')
  eq(chipLabel('ctrl+comma'), '⌃,', 'chip for a named punctuation key')

  /* ---------------------------------------------------------------- when-clause grammar */

  eq(evaluateWhen(null, {}), true, 'no clause always holds')
  eq(evaluateWhen('  ', {}), true, 'an empty clause always holds')
  eq(evaluateWhen('a && b', { a: true, b: true }), true, 'conjunction')
  eq(evaluateWhen('a && b', { a: true }), false, 'conjunction with a missing flag')
  eq(evaluateWhen('a || b', { b: true }), true, 'disjunction')
  eq(evaluateWhen('!a', {}), true, 'negation of an unknown flag')
  eq(evaluateWhen('a && !b || c', { a: true, b: true }), false, '&& binds tighter than ||')
  eq(evaluateWhen('a && (!b || c)', { a: true, c: true, b: true }), true, 'parentheses')
  eq(evaluateWhen('a &&', { a: true }), false, 'a truncated clause never fires')
  eq(evaluateWhen('a == b', { a: true }), false, 'an unsupported operator never fires')

  /* ------------------------------------------------------------------ keymap resolution */

  {
    // Later bindings win: that is how a user override shadows a default.
    const km = buildKeymap([
      { key: 'ctrl+p', command: 'picker.files', when: null },
      { key: 'ctrl+p', command: 'terminal.paste', when: null },
    ])
    const r = km.resolve('ctrl+p', {})
    eq(r.kind, 'run', 'a shadowed key still resolves')
    eq(r.command, 'terminal.paste', 'the last applicable binding wins')
  }
  {
    // A `-command` removal directive is Rust's business and must never bind anything here.
    const km = buildKeymap([{ key: 'ctrl+p', command: '-picker.files', when: null }])
    eq(km.resolve('ctrl+p', {}).kind, 'unbound', 'a removal directive binds nothing')
  }
  {
    // A prefix whose only continuation is gated off must not swallow the stroke.
    const km = buildKeymap([
      { key: 'ctrl+k ctrl+s', command: 'settings.keymap', when: 'editorFocused' },
    ])
    eq(km.resolve('ctrl+k', {}).kind, 'unbound', 'an inapplicable prefix does not arm')
    eq(km.resolve('ctrl+k', { editorFocused: true }).kind, 'prefix', 'an applicable prefix arms')
  }
  {
    const km = buildKeymap(BINDINGS)
    eq(km.keyFor('settings.keymap'), 'ctrl+k ctrl+s', 'keyFor finds a sequence')
    eq(km.chipFor('picker.files'), '⌃P', 'chipFor renders the chip')
    eq(km.chipFor('nope.nothing'), null, 'chipFor is null for an unbound command')
    // Destructured, because a caller writing this must not get a `this` error.
    const { chipFor } = km
    eq(chipFor('palette.commands'), '⌃⇧P', 'chipFor works detached from its object')
  }

  /* -------------------------------------------------- every bound key reaches a dispatcher */

  /*
   * A binding whose command nothing dispatches is worse than no binding at all.
   *
   * `ctrl+s` was bound to `file.save` in `cide_core::keymap::defaults()` from the day the
   * table was written, and nothing in the frontend had a case for it. The gate did exactly
   * what it is supposed to do — resolved the chord, swallowed the keystroke, called the
   * dispatcher — and the dispatcher logged a line and returned. The effect was that Ctrl+S
   * did nothing *and* that CodeMirror's own `Mod-s`, which would have saved the file, never
   * saw the event. Every assertion above passed throughout: they check that a chord resolves
   * and dispatches, not that anything is listening.
   *
   * So this reads the two files that dispatch — `keys/dispatch.ts` and the app's fallback in
   * `App.tsx` — and asserts that every command in the shipped table reaches one of them.
   * Source text rather than an import because `App.tsx` cannot be compiled here (it is the
   * whole application) and because the question is genuinely "is this string written down as
   * a case", which is what a `switch` is.
   */
  {
    const readSource = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
    const dispatchers = readSource('../src/keys/dispatch.ts') + readSource('../src/App.tsx')
    const dispatched = new Set()
    for (const [, id] of dispatchers.matchAll(/case '([a-zA-Z][\w.]*)':/g)) dispatched.add(id)
    for (const [, id] of dispatchers.matchAll(/command === '([a-zA-Z][\w.]*)'/g)) dispatched.add(id)
    ok(dispatched.size >= 10, `found ${dispatched.size} dispatched commands — the scan still works`)

    /*
     * The bindings that are still dead, each with the reason, so that a *new* one cannot be
     * added without either wiring it or arguing for a line here.
     *
     * None of these is in this file's gift: they need the workspace store and `App.tsx`,
     * which belongs to whoever is merging. They are listed rather than quietly excluded
     * because the whole point of the assertion is that a dead key is a fact somebody has to
     * look at.
     */
    const KNOWN_DEAD = {
      'tab.close': 'needs the focused tab and `tab_close`; the × button is the only way today',
      'theme.toggle': 'the app header has the toggle; no command reaches it',
      'settings.open': 'the activity rail’s ⚙ is the only gesture that opens Settings',
      'pane.promoteToTab': 'no dispatcher; `pane_promote` is wired to nothing',
      'pane.detachToWindow': 'no dispatcher; the pane menu is the only route',
      'pane.navigate.left': 'no dispatcher for the `pane_navigate` family',
      'pane.navigate.right': 'no dispatcher for the `pane_navigate` family',
      'pane.navigate.up': 'no dispatcher for the `pane_navigate` family',
      'pane.navigate.down': 'no dispatcher for the `pane_navigate` family',
    }

    for (const { key, command } of rustDefaults()) {
      if (Object.hasOwn(KNOWN_DEAD, command)) continue
      ok(
        dispatched.has(command),
        `${key} is bound to ${command} and something dispatches it ` +
          '(a bound key whose command nobody handles swallows the keystroke and does nothing)',
      )
    }
    // And the list cannot rot: wiring one of them without deleting its line here would leave
    // a note claiming a gap that no longer exists.
    for (const command of Object.keys(KNOWN_DEAD)) {
      ok(!dispatched.has(command), `${command} is still undispatched — remove it from KNOWN_DEAD`)
    }
  }

  if (failed > 0) {
    console.error(`\ncheck-key-gate: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`check-key-gate: ok (${compared} chords through both entry points)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}
