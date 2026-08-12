/**
 * The held-modifier project switcher: the whole gesture, keypress by keypress.
 *
 * Ctrl+Tab is not a two-item toggle and not a step through the header strip. It is the
 * Windows / IDEA / browser switcher, and the four things that make it that are the four things
 * this script proves:
 *
 *   1. **one press-and-release toggles** — press Ctrl+Tab, let go, and you are on the project
 *      you were on before, with the stack now the other way round;
 *   2. **three Tabs held lands on MRU[3]** — the walk keeps going down the whole list rather
 *      than flip-flopping between the top two, which is what the user asked for in as many
 *      words;
 *   3. **Escape leaves the order untouched** — cancelling is not a quiet commit;
 *   4. **a lost keyup cancels** — alt-tabbing away while Ctrl is down must not leave a popup
 *      up for ever swallowing every Tab afterwards.
 *
 * `src/keys/switcher.ts` is written so that all four are arithmetic: the walk holds a frozen
 * list and an index, and `commit` is the only function in it that produces a new stack. So the
 * first half of this script compiles that module and drives it, exactly as
 * `check-status-format.mjs` and `check-key-gate.mjs` do with their pure modules — there is no
 * JS test runner here and adding one for this would be a larger commitment than the code.
 *
 * The second half is a source scan of `src/keys/switcherStore.ts`, which cannot be compiled
 * standalone (zustand, the IPC client, the workspace mirror). It is not a stylistic check: the
 * store is where a walk is armed and where it must be *disarmed*, and the failure mode is
 * silent and permanent — a walk with no way to end swallows Tab in every pane for the rest of
 * the session. There is no DOM in this process to press a key against, so the proof that the
 * three ways out are wired is that the listeners are registered and removed together.
 *
 * Run: `pnpm --dir ui run check:switcher`
 */
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const out = mkdtempSync(join(tmpdir(), 'cide-switcher-'))
let failed = 0

const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (!cond) fail(what)
}

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/keys/chords.ts',
      'src/keys/switcher.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'commonjs',
      '--moduleResolution', 'node10',
      '--target', 'es2022',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
      // Neither module names a DOM type — `ModifierState` is structural precisely so this one
      // stays free of them. The lib is here because `tsc` auto-includes every `@types` package
      // in `node_modules`, and `@types/react-dom` does not compile without it.
      '--lib', 'es2023,dom',
    ],
    { stdio: 'inherit' },
  )

  const require = createRequire(import.meta.url)
  const {
    advance,
    begin,
    capture,
    commit,
    holdOf,
    reconcile,
    restack,
    selection,
    stillHeld,
    touch,
  } = require(join(out, 'keys/switcher.js'))

  /* ------------------------------------------------------------------ the harness */

  /** Every modifier down, as a `KeyboardEvent`-shaped thing. */
  const held = (mods = {}) => ({
    ctrlKey: mods.ctrl !== false,
    altKey: mods.alt === true,
    metaKey: mods.meta === true,
  })
  const released = { ctrlKey: false, altKey: false, metaKey: false }

  /**
   * One switcher session, driven the way the app drives it.
   *
   * The three inputs are the three the app has: the command (`press`), a keystroke arriving at
   * the key gate's capture (`stroke`), and the release watcher firing (`keyup`, `lost`). The
   * transitions are the compiled module's; what is written here is only the wiring
   * `keys/switcherStore.ts` does, so a change to a rule is felt in every scenario below.
   */
  const session = (stack, options = {}) => {
    const live = options.live ?? stack
    let order = [...stack]
    let walk = null
    const activated = []

    const end = (commitIt) => {
      if (walk === null) return
      if (commitIt) {
        const result = commit(walk, live)
        order = result.order
        if (result.selected !== null) activated.push(result.selected)
      }
      walk = null
    }

    return {
      order: () => order,
      walk: () => walk,
      activated,
      selected: () => (walk === null ? null : selection(walk)),

      /** `project.switcher.next` / `.prev`, dispatched from the key gate or the palette. */
      press(step, stroke) {
        if (walk !== null) {
          walk = advance(walk, step)
          return
        }
        const outcome = begin(order, holdOf(stroke ?? null), step)
        if (outcome.kind === 'walk') walk = outcome.walk
        else if (outcome.kind === 'activate') {
          activated.push(outcome.id)
          order = touch(order, outcome.id)
        }
      },

      /** A keystroke reaching the gate's capture while the walk is open. */
      stroke(text) {
        if (walk === null) return { kind: 'ignore', consumed: false }
        const action = capture(walk, text)
        if (action.kind === 'advance') walk = advance(walk, action.step)
        else if (action.kind === 'cancel') end(false)
        return action
      },

      /** The release watcher's keyup branch. */
      keyup(state) {
        if (walk !== null && !stillHeld(walk.hold, state)) end(true)
      },

      /** Its `blur` / `visibilitychange` branch — the lost keyup. */
      lost() {
        end(false)
      },
    }
  }

  const STACK = ['alpha', 'beta', 'gamma', 'delta']

  /* ------------------------------- 1. one press-and-release is the two-item toggle */

  {
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    eq(s.selected(), 'beta', 'the first Ctrl+Tab selects MRU[1], the previously used project')
    eq(s.order(), STACK, 'and nothing is reordered while the modifier is still down')
    s.keyup(released)
    eq(s.activated, ['beta'], 'releasing Ctrl activates the selection')
    eq(s.order(), ['beta', 'alpha', 'gamma', 'delta'], 'and moves it to the front of the stack')
    eq(s.walk(), null, 'and closes the popup')

    // The toggle half: doing it again comes straight back.
    s.press(1, 'ctrl+tab')
    s.keyup(released)
    eq(s.activated, ['beta', 'alpha'], 'a second press-and-release toggles back')
    eq(s.order(), STACK, 'and the stack is where it started')
  }

  /* --------------------------------------- 2. three Tabs held lands on MRU[3] */

  {
    /*
     * The claim in the user's words: "until CTRL is pressed and pressing TAB twice - should
     * follow to iteration between whole list". A toggle would sit on `beta` for ever; a walk
     * over a list that reorders as you go would land on `alpha`, because the first step would
     * have promoted `beta` under the highlight.
     */
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    eq(s.selected(), 'beta', 'Tab 1 → MRU[1]')
    eq(s.stroke('ctrl+tab').consumed, true, 'the second Tab is the switcher’s, not the keymap’s')
    eq(s.selected(), 'gamma', 'Tab 2 → MRU[2]')
    s.stroke('ctrl+tab')
    eq(s.selected(), 'delta', 'Tab 3 → MRU[3] — the whole list, not a toggle')
    eq(s.order(), STACK, 'and still nothing has been reordered')

    s.keyup(released)
    eq(s.activated, ['delta'], 'the release activates the third one down')
    eq(s.order(), ['delta', 'alpha', 'beta', 'gamma'], 'and only then does the stack move')
  }

  {
    // It wraps, in both directions, because the list is a ring.
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    for (let i = 0; i < 3; i++) s.stroke('ctrl+tab')
    eq(s.selected(), 'alpha', 'a fourth Tab wraps back to the active project')

    const back = session(STACK)
    back.press(-1, 'ctrl+shift+tab')
    eq(back.selected(), 'delta', 'Ctrl+Shift+Tab starts at the least recently used')
    eq(back.stroke('ctrl+shift+tab').step, -1, 'and Shift+Tab walks backwards')
    eq(back.selected(), 'gamma', 'one back from the end')
  }

  {
    // Shift+Tab mid-walk reverses without closing. `ctrl+shift+tab` and `ctrl+tab` are the
    // same gesture with the direction flipped, so they must not be two different states.
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    s.stroke('ctrl+tab')
    eq(s.selected(), 'gamma', 'forwards twice')
    s.stroke('ctrl+shift+tab')
    eq(s.selected(), 'beta', 'and back one')
    eq(s.walk() === null, false, 'the walk is still open')
  }

  /* ------------------------------------------ 3. Escape leaves the order untouched */

  {
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    s.stroke('ctrl+tab')
    s.stroke('ctrl+tab')
    eq(s.selected(), 'delta', 'walked to the far end')

    const action = s.stroke('ctrl+escape')
    eq(action.kind, 'cancel', 'Escape cancels')
    eq(action.consumed, true, 'and is swallowed — it must not reach a terminal as ^[')
    eq(s.walk(), null, 'the popup closed')
    eq(s.activated, [], 'nothing was activated')
    eq(s.order(), STACK, 'and the stack is byte-for-byte what it was')

    // A late keyup after cancelling must not resurrect the commit.
    s.keyup(released)
    eq(s.activated, [], 'a keyup after Escape activates nothing')
    eq(s.order(), STACK, 'and still reorders nothing')
  }

  {
    // Unmodified Escape too: whether the keyboard reports `escape` or `ctrl+escape` while
    // Ctrl is down is not something to depend on.
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    eq(s.stroke('escape').consumed, true, 'a bare Escape cancels and is swallowed too')
    eq(s.order(), STACK, 'order untouched')
  }

  /* ------------------------------------------------------ 4. a lost keyup cancels */

  {
    // Alt+Tab to another application with Ctrl still down: the keyup is delivered over there
    // and never arrives here. `blur` is the only thing that ends the walk.
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    s.stroke('ctrl+tab')
    eq(s.selected(), 'gamma', 'mid-walk when focus leaves')
    s.lost()
    eq(s.walk(), null, 'blur closes the walk')
    eq(s.activated, [], 'and activates nothing — the gesture was interrupted, not completed')
    eq(s.order(), STACK, 'so the stack is untouched')

    // And the capture is gone with it: Tab is an ordinary keystroke again.
    eq(s.stroke('ctrl+tab').consumed, false, 'Tab is no longer swallowed once the walk is over')
  }

  {
    /*
     * The other half of the same failure, and the one no `blur` fires for: the modifier came
     * up while the window was not listening, and the next thing we see is a keystroke that
     * could not have been typed with Ctrl down. That cancels — and is *not* consumed, because
     * it is a real keystroke the user meant for something else.
     */
    const s = session(STACK)
    s.press(1, 'ctrl+tab')
    const action = s.stroke('a')
    eq(action.kind, 'cancel', 'a stroke without the hold reveals the modifier is up')
    eq(action.consumed, false, 'and passes through to whatever it was for')
    eq(s.walk(), null, 'the walk is closed')
    eq(s.order(), STACK, 'with no reordering')
  }

  {
    /*
     * A rebound multi-modifier hold. The rule is "every modifier named by the opening chord is
     * still down", so letting go of *any* of them commits — the user has stopped making the
     * gesture, and the alternative ("wait for all of them") leaves the popup up on the far more
     * likely mistake of releasing Ctrl first and Alt a beat later.
     */
    const s = session(STACK)
    s.press(1, 'ctrl+alt+tab')
    eq(s.selected(), 'beta', 'a rebound ctrl+alt+tab opens the same walk')
    eq(holdOf('ctrl+alt+tab'), { ctrl: true, alt: true, meta: false }, 'and holds both modifiers')
    s.keyup(held({ ctrl: true, alt: true }))
    eq(s.walk() === null, false, 'with both still down, a keyup commits nothing')
    s.keyup(held({ ctrl: true, alt: false }))
    eq(s.activated, ['beta'], 'letting go of either of them commits')
  }

  {
    // Shift is the direction, never part of the hold: letting go of it mid-walk must not
    // commit, or Ctrl+Shift+Tab could never be followed by Ctrl+Tab.
    eq(holdOf('ctrl+shift+tab'), { ctrl: true, alt: false, meta: false }, 'shift is not held')
    eq(stillHeld({ ctrl: true, alt: false, meta: false }, held()), true, 'ctrl alone is enough')
  }

  /* ------------------------------------------------- the palette, and the degenerate cases */

  {
    // No key, no modifier, no popup — and it still terminates. A walk that waited for the
    // release of a modifier nobody is holding would never close.
    const s = session(STACK)
    s.press(1, null)
    eq(s.walk(), null, 'the palette opens no popup')
    eq(s.activated, ['beta'], 'it switches to the most recently used project immediately')
    eq(s.order(), ['beta', 'alpha', 'gamma', 'delta'], 'and reorders on the spot')

    const back = session(STACK)
    back.press(-1, null)
    eq(back.activated, ['delta'], 'and backwards from the palette is the least recently used')
  }

  eq(begin(['only'], { ctrl: true, alt: false, meta: false }, 1), { kind: 'nothing' },
    'one project is nowhere to switch to')
  eq(begin([], null, 1), { kind: 'nothing' }, 'and neither is none')
  eq(holdOf('tab'), null, 'a bare key holds nothing')
  eq(holdOf('shift+tab'), null, 'and neither does shift on its own')

  /* --------------------------------------------- the stack survives opening and closing */

  {
    eq(touch(['a', 'b', 'c'], 'c'), ['c', 'a', 'b'], 'touch moves to the front')
    eq(touch(['a', 'b'], 'z'), ['z', 'a', 'b'], 'and a project it has never seen goes in front')
    eq(reconcile(['a', 'b', 'c'], ['a', 'c']), ['a', 'c'], 'a closed project leaves the stack')
    eq(reconcile(['b', 'a'], ['a', 'b', 'c']), ['b', 'a', 'c'], 'a new one arrives at the back')
    eq(reconcile([], ['a', 'b']), ['a', 'b'], 'an empty stack seeds from header order')
    eq(reconcile(['x'], ['a']), ['a'], 'a stack of only stale ids is replaced wholesale')
  }

  {
    /*
     * `restack` — the snapshot rule, and specifically **which windows are allowed to speak**.
     *
     * The stack is persisted to a `localStorage` key shared by every window of the origin, so
     * this is the one place where a window that has nothing to do with Ctrl+Tab can destroy the
     * order the user actually built. A detached tab or pane window's strip is `[]`
     * (`keys/target.ts`), and in `WindowMode::PerProject` every shell window's strip holds one
     * project — reconciling against either would answer "everything closed".
     */
    const stack = ['alpha', 'beta', 'gamma']
    eq(restack(stack, ['alpha', 'beta', 'gamma'], 'gamma'), ['gamma', 'alpha', 'beta'],
      'a shell window with a walkable strip promotes its active project')
    eq(restack(stack, ['alpha', 'gamma'], 'alpha'), ['alpha', 'gamma'],
      'and drops a project that really did close')
    eq(restack(stack, ['alpha', 'beta', 'gamma', 'delta'], 'alpha'), [...stack, 'delta'],
      'a project opened elsewhere arrives at the back')

    ok(restack(stack, [], null) === stack,
      'a detached window — strip `[]` — leaves the stack alone, identity and all: it shares the ' +
        'cache with the shell window and must not wipe it')
    ok(restack(stack, ['beta'], 'beta') === stack,
      'and neither may a PerProject shell window, whose strip holds one project')
    ok(restack(stack, ['alpha', 'beta', 'gamma'], 'alpha') === stack,
      'an unchanged order keeps its identity, so nothing is written back for a no-op')
  }

  {
    // A project closing *during* a walk. The frozen order still names it; the commit must not.
    const s = session(STACK, { live: ['alpha', 'beta', 'delta'] })
    s.press(1, 'ctrl+tab')
    s.stroke('ctrl+tab')
    eq(s.selected(), 'gamma', 'the walk still shows the project that has just closed')
    s.keyup(released)
    eq(s.activated, [], 'releasing on it activates nothing')
    eq(s.order(), ['alpha', 'beta', 'delta'], 'and the stack comes back reconciled')
  }

  {
    // Releasing on a project that is still there, while another one closed underneath.
    const s = session(STACK, { live: ['alpha', 'beta', 'delta'] })
    s.press(1, 'ctrl+tab')
    s.keyup(released)
    eq(s.activated, ['beta'], 'the surviving selection is activated')
    eq(s.order(), ['beta', 'alpha', 'delta'], 'and the closed project is gone from the stack')
  }

  /* ------------------------------------------------------------ the store's own wiring */

  /*
   * Everything above is arithmetic and cannot tell you whether anything calls it. These are
   * the four claims about `keys/switcherStore.ts` that no pure test can make, each of them
   * standing for a way the gesture fails silently and permanently.
   */
  {
    const store = read('../src/keys/switcherStore.ts')
    const code = store.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

    // 1. The three ways out are all wired. Miss one and the popup is immortal.
    for (const [listener, why] of [
      ["addEventListener('keyup'", 'the ordinary release'],
      ["addEventListener('blur'", 'alt-tabbing away with the modifier still down'],
      ["addEventListener('visibilitychange'", 'the window being hidden without a blur'],
    ]) {
      ok(code.includes(listener), `switcherStore watches for ${why} (${listener})`)
    }
    for (const listener of ['keyup', 'blur', 'visibilitychange']) {
      ok(
        code.includes(`removeEventListener('${listener}'`),
        `and removes its ${listener} listener when the walk ends — a leaked one swallows the next Tab`,
      )
    }

    // 2. The release is a latch, not a third way to resolve a chord. `keys/gate.ts` resolves
    //    on keydown at two entry points that `check-key-gate.mjs` proves agree; a keyup route
    //    into the keymap would be exactly the asymmetry that check exists to catch.
    ok(
      !/from '\.\/keymap'|buildKeymap|\.resolve\(/.test(code),
      'switcherStore resolves no chords — the release watcher is a modifier latch, not an entry point',
    )
    ok(
      code.includes('stillHeld(walk.hold, ev)'),
      'and decides purely, through `stillHeld`, rather than hard-coding `ev.ctrlKey`',
    )

    // 3. Cancelling writes nothing. The whole point of committing on release.
    const cancel = code.slice(code.indexOf('export function cancelWalk'), code.indexOf('export function commitWalk'))
    ok(cancel.length > 0, 'found cancelWalk')
    ok(
      !cancel.includes('rememberMru') && !cancel.includes('activateProject') && !cancel.includes('commit('),
      'cancelWalk activates nothing and writes no order — Escape cannot reorder the stack',
    )

    // 4. And committing does both, in that order.
    const commitBody = code.slice(code.indexOf('export function commitWalk'), code.indexOf('export function startProjectSwitch'))
    ok(commitBody.includes('rememberMru('), 'commitWalk records the new order')
    ok(commitBody.includes('activateProject('), 'and activates the selection')
    ok(
      commitBody.indexOf('rememberMru(') < commitBody.indexOf('activateProject('),
      'order before activation, so a second Ctrl+Tab inside the round trip reads the new stack',
    )
  }

  {
    /*
     * The hold comes from the *keystroke that dispatched the command*, and nowhere else.
     *
     * This one line is what separates the two behaviours: with a stroke, Ctrl+Tab opens a walk
     * that waits for the release; with `null` it degenerates to an immediate switch — a
     * two-item toggle, which is precisely what the user rejected. `check-key-gate.mjs` proves
     * the gate publishes the stroke; this proves the store still asks for it.
     */
    const store = read('../src/keys/switcherStore.ts')
    ok(
      /begin\([^)]*holdOf\(currentStroke\(\)\)/.test(store),
      'startProjectSwitch reads the hold off the dispatching stroke — a constant here would ' +
        'turn Ctrl+Tab back into the toggle the switcher replaced',
    )
    // And the snapshot rule is the shared one, so the guard on which windows may rewrite the
    // stack cannot be re-inlined without the assertions above noticing.
    const ws = read('../src/store/workspace.ts')
    ok(/restack\(/.test(ws), 'the workspace store derives its stack through `restack`')
  }

  {
    // The gate has to actually consult the capture, or the popup is decorative and Tab still
    // reaches the shell underneath it.
    const wiring = read('../src/keys/useKeyGate.ts')
    ok(
      /capture:\s*switcherCapture/.test(wiring),
      'useKeyGate hands the switcher’s capture to the gate',
    )
    const gate = read('../src/keys/gate.ts')
    ok(
      gate.includes('host.capture?.(stroke)'),
      'and the gate consults it',
    )
    ok(
      /pendingSequence === null && host\.capture/.test(gate),
      'only when no chord prefix is armed — an armed prefix outranks the capture',
    )
  }

  {
    // The default binding is the switcher's, read out of the Rust that defines it.
    const keymapRs = read('../../crates/cide-core/src/keymap.rs')
    const body = keymapRs.slice(keymapRs.indexOf('pub fn defaults() -> Vec<Binding> {'))
    const bound = Object.fromEntries(
      [...body.slice(0, body.indexOf('\n}')).matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => [m[1], m[2]]),
    )
    eq(bound['ctrl+tab'], 'project.switcher.next', 'ctrl+tab is bound to the switcher')
    eq(bound['ctrl+shift+tab'], 'project.switcher.prev', 'and ctrl+shift+tab walks it back')
  }

  if (failed > 0) {
    console.error(`\ncheck-switcher: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-switcher: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
