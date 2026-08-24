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
 *      up for ever swallowing every Tab afterwards;
 *   5. **the release commits on the keyup this app really receives** — WebKitGTK reports Ctrl
 *      as still down in the event that releases it, so every release below is driven in that
 *      shape as well as the browser one. Getting this wrong is not subtle: the switcher opens,
 *      the user lets go, and nothing happens.
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
    endsHold,
    hintFor,
    openingOf,
    reconcile,
    restack,
    selection,
    stillHeld,
    touch,
  } = require(join(out, 'keys/switcher.js'))

  /* ------------------------------------------------------------------ the harness */

  /**
   * One keyup, as a `KeyboardEvent`-shaped thing: the key that came *up*, and the modifier
   * flags the event reports as still down.
   *
   * They are given separately because on WebKitGTK they contradict each other — see
   * `endsHold`. `up('Control', { ctrl: true })` is not a nonsense event, it is literally what
   * the shipped app receives when the user lets go of Ctrl.
   */
  const up = (key, mods = {}) => ({
    key,
    ctrlKey: mods.ctrl === true,
    altKey: mods.alt === true,
    metaKey: mods.meta === true,
  })

  /** Letting go of Ctrl, in the shape this app's own webview delivers it. */
  const released = up('Control', { ctrl: true })

  /**
   * One switcher session, driven the way the app drives it.
   *
   * The three inputs are the three the app has: the command (`press`), a keystroke arriving at
   * the key gate's capture (`stroke`), and the release watcher firing (`keyup`, `lost`). The
   * transitions are the compiled module's; what is written here is only the wiring
   * `keys/switcherStore.ts` does, so a change to a rule is felt in every scenario below.
   */
  const session = (stack, options = {}) => {
    // Two live lists, because the interesting failures are the ones where they differ.
    // `atOpen` is what exists when the popup goes up — a stack read from `localStorage` can name
    // ids that are already gone — and `live` is what exists when the modifier comes up, which is
    // how "a tab closed in another window mid-walk" is expressed. Both default to the stack, so
    // an ordinary scenario says nothing about either.
    const atOpen = options.liveAtOpen ?? stack
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

      /** The `*.switcher.next` / `.prev` command, from the key gate or from the palette. */
      press(step, stroke) {
        if (walk !== null) {
          walk = advance(walk, step)
          return
        }
        // `reconcile` before `begin`, exactly as `startSwitch` does it: a remembered stack that
        // has drifted from the live list must not be what the walk freezes.
        const outcome = begin(reconcile(order, atOpen), openingOf(stroke ?? null), step)
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
      keyup(ev) {
        if (walk !== null && endsHold(walk.hold, ev)) end(true)
      },

      /** Its `blur` / `visibilitychange` branch — the lost keyup. */
      lost() {
        end(false)
      },
    }
  }

  const STACK = ['alpha', 'beta', 'gamma', 'delta']

  /*
   * The whole scenario table below runs **twice**, once per shipped switcher.
   *
   * Ctrl+Tab walks tabs and Ctrl+` walks projects, and they are one implementation over opaque
   * id strings — so running the table twice is not a second suite, it is a loop, and what the
   * second pass actually buys is the one axis that genuinely differs: the **walk key**. Every
   * `capture` assertion in here compares against `walk.key`, and with the key hard-coded back to
   * `'tab'` (which is what it was until M14) the `backquote` pass fails on the strokes that
   * matter — the second press falls through to the keymap and the reverse stroke resolves to
   * `terminal.splitBelow`.
   *
   * The ids stay `alpha`…`delta` in both passes, deliberately. They are opaque to every function
   * under test, and naming them `t1`/`p1` per pass would suggest the arithmetic can tell.
   */
  for (const KEY of ['tab', 'backquote']) {
    const FWD = `ctrl+${KEY}`
    const BACK = `ctrl+shift+${KEY}`
    /** A rebound multi-modifier opener, for the hold rules. */
    const HELD = `ctrl+alt+${KEY}`


    /* ------------------------------- 1. one press-and-release is the two-item toggle */

    {
      const s = session(STACK)
      s.press(1, FWD)
      eq(s.selected(), 'beta', 'the first press selects MRU[1], the one used before this')
      eq(s.order(), STACK, 'and nothing is reordered while the modifier is still down')
      s.keyup(released)
      eq(s.activated, ['beta'], 'releasing Ctrl activates the selection')
      eq(s.order(), ['beta', 'alpha', 'gamma', 'delta'], 'and moves it to the front of the stack')
      eq(s.walk(), null, 'and closes the popup')

      // The toggle half: doing it again comes straight back.
      s.press(1, FWD)
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
      s.press(1, FWD)
      eq(s.selected(), 'beta', 'press 1 → MRU[1]')
      eq(s.stroke(FWD).consumed, true, 'the second press is the switcher’s, not the keymap’s')
      eq(s.selected(), 'gamma', 'press 2 → MRU[2]')
      s.stroke(FWD)
      eq(s.selected(), 'delta', 'press 3 → MRU[3] — the whole list, not a toggle')
      eq(s.order(), STACK, 'and still nothing has been reordered')

      s.keyup(released)
      eq(s.activated, ['delta'], 'the release activates the third one down')
      eq(s.order(), ['delta', 'alpha', 'beta', 'gamma'], 'and only then does the stack move')
    }

    {
      // It wraps, in both directions, because the list is a ring.
      const s = session(STACK)
      s.press(1, FWD)
      for (let i = 0; i < 3; i++) s.stroke(FWD)
      eq(s.selected(), 'alpha', 'a fourth press wraps back to the active one')

      const back = session(STACK)
      back.press(-1, BACK)
      eq(back.selected(), 'delta', 'the reverse command starts at the least recently used')
      eq(back.stroke(BACK).step, -1, 'and Shift walks backwards')
      eq(back.selected(), 'gamma', 'one back from the end')
    }

    {
      // Shift+Tab mid-walk reverses without closing. `ctrl+shift+tab` and `ctrl+tab` are the
      // same gesture with the direction flipped, so they must not be two different states.
      const s = session(STACK)
      s.press(1, FWD)
      s.stroke(FWD)
      eq(s.selected(), 'gamma', 'forwards twice')
      s.stroke(BACK)
      eq(s.selected(), 'beta', 'and back one')
      eq(s.walk() === null, false, 'the walk is still open')
    }

    /* ------------------------------------------ 3. Escape leaves the order untouched */

    {
      const s = session(STACK)
      s.press(1, FWD)
      s.stroke(FWD)
      s.stroke(FWD)
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
      s.press(1, FWD)
      eq(s.stroke('escape').consumed, true, 'a bare Escape cancels and is swallowed too')
      eq(s.order(), STACK, 'order untouched')
    }

    /* ------------------------------------------------------ 4. a lost keyup cancels */

    {
      // Alt+Tab to another application with Ctrl still down: the keyup is delivered over there
      // and never arrives here. `blur` is the only thing that ends the walk.
      const s = session(STACK)
      s.press(1, FWD)
      s.stroke(FWD)
      eq(s.selected(), 'gamma', 'mid-walk when focus leaves')
      s.lost()
      eq(s.walk(), null, 'blur closes the walk')
      eq(s.activated, [], 'and activates nothing — the gesture was interrupted, not completed')
      eq(s.order(), STACK, 'so the stack is untouched')

      // And the capture is gone with it: Tab is an ordinary keystroke again.
      eq(s.stroke(FWD).consumed, false, 'the walk key is no longer swallowed once the walk is over')
    }

    {
      /*
       * The other half of the same failure, and the one no `blur` fires for: the modifier came
       * up while the window was not listening, and the next thing we see is a keystroke that
       * could not have been typed with Ctrl down. That cancels — and is *not* consumed, because
       * it is a real keystroke the user meant for something else.
       */
      const s = session(STACK)
      s.press(1, FWD)
      const action = s.stroke('a')
      eq(action.kind, 'cancel', 'a stroke without the hold reveals the modifier is up')
      eq(action.consumed, false, 'and passes through to whatever it was for')
      eq(s.walk(), null, 'the walk is closed')
      eq(s.order(), STACK, 'with no reordering')
    }

    /* --------------------------------------- 5. the release, in both platform shapings */

    {
      /*
       * The bug this section exists for, in one gesture: press Ctrl+Tab, let go of Ctrl, and the
       * selected project opens.
       *
       * It read as a freeze — the popup stayed up, the hint went on offering Tab and Shift+Tab,
       * and only Escape or a click got rid of it — because the release watcher asked
       * `!stillHeld(hold, ev)` and WebKitGTK reports `ctrlKey === true` in the very keyup that
       * releases Ctrl (GDK's state is the mask from *before* the event, and WebKit only patches
       * the press side of that). Both shapings are driven here so neither platform's answer can
       * be the accidental one.
       */
      const gtk = session(STACK)
      gtk.press(1, FWD)
      gtk.keyup(up('Control', { ctrl: true }))
      eq(gtk.activated, ['beta'], 'WebKitGTK: the Ctrl keyup commits even though it claims Ctrl is down')
      eq(gtk.walk(), null, 'and the popup goes away with it')

      const web = session(STACK)
      web.press(1, FWD)
      web.keyup(up('Control'))
      eq(web.activated, ['beta'], 'Chrome/Firefox: the same keyup with honest flags commits too')

      const walking = session(STACK)
      walking.press(1, FWD)
      walking.keyup(up('Tab', { ctrl: true }))
      eq(walking.activated, [], 'the Tab coming up mid-walk activates nothing')
      eq(walking.walk() === null, false, 'and leaves the popup where it is')

      // The fallback still earns its place: a hold released while the window was not listening,
      // revealed by the flags on some later keyup that is not a modifier at all.
      const late = session(STACK)
      late.press(1, FWD)
      late.keyup(up('a'))
      eq(late.activated, ['beta'], 'a keyup that reveals the hold is gone commits as well')
    }

    {
      /*
       * A rebound multi-modifier hold. The rule is "every modifier named by the opening chord is
       * still down", so letting go of *any* of them commits — the user has stopped making the
       * gesture, and the alternative ("wait for all of them") leaves the popup up on the far more
       * likely mistake of releasing Ctrl first and Alt a beat later.
       */
      const s = session(STACK)
      s.press(1, HELD)
      eq(s.selected(), 'beta', 'a rebound ctrl+alt+tab opens the same walk')
      eq(
        openingOf(HELD),
        { hold: { ctrl: true, alt: true, meta: false }, key: KEY },
        'and holds both modifiers — and remembers the key it was opened on, which is what the ' +
          'capture claims for as long as the popup is up',
      )
      s.keyup(up('Tab', { ctrl: true, alt: true }))
      eq(s.walk() === null, false, 'the Tab coming up is not a release: both modifiers are still down')
      // Alt is the one let go of, and the flags — WebKitGTK's, from before the event — still
      // claim it is down. The key that came up is what decides.
      s.keyup(up('Alt', { ctrl: true, alt: true }))
      eq(s.activated, ['beta'], 'letting go of either of them commits')
    }

    {
      // Shift is the direction, never part of the hold: letting go of it mid-walk must not
      // commit, or Ctrl+Shift+Tab could never be followed by Ctrl+Tab.
      const hold = { ctrl: true, alt: false, meta: false }
      eq(openingOf(BACK)?.hold, hold, 'shift is not held')
      eq(stillHeld(hold, up('Shift', { ctrl: true })), true, 'ctrl alone is enough')
      eq(endsHold(hold, up('Shift', { ctrl: true })), false, 'so releasing Shift is not a release')

      const s = session(STACK)
      s.press(-1, BACK)
      s.keyup(up('Shift', { ctrl: true }))
      eq(s.activated, [], 'the walk survives it')
      eq(s.selected(), 'delta', 'still on the same project')
      eq(s.stroke(FWD).consumed, true, 'and the walk key still walks forwards from there')
    }

    /* ------------------------------------------------- the palette, and the degenerate cases */

    {
      // No key, no modifier, no popup — and it still terminates. A walk that waited for the
      // release of a modifier nobody is holding would never close.
      const s = session(STACK)
      s.press(1, null)
      eq(s.walk(), null, 'the palette opens no popup')
      eq(s.activated, ['beta'], 'it switches to the most recently used entry immediately')
      eq(s.order(), ['beta', 'alpha', 'gamma', 'delta'], 'and reorders on the spot')

      const back = session(STACK)
      back.press(-1, null)
      eq(back.activated, ['delta'], 'and backwards from the palette is the least recently used')
    }

    eq(begin(['only'], { hold: { ctrl: true, alt: false, meta: false }, key: KEY }, 1),
      { kind: 'nothing' }, 'one entry is nowhere to switch to')
    eq(begin([], null, 1), { kind: 'nothing' }, 'and neither is none')
    eq(openingOf(KEY), null, 'a bare key holds nothing')
    eq(openingOf(`shift+${KEY}`), null, 'and neither does shift on its own')

    {
      /*
       * The footer line names the keys that are actually in play.
       *
       * It read "Release Ctrl to switch · Tab / Shift+Tab to move" as a literal, which was true
       * of the one switcher that existed and is a lie about the other — and about either one
       * after a rebind. The popup is the *only* place the gesture is explained, so a hint naming
       * a key that does nothing is worse than no hint.
       */
      const opened = begin(STACK, openingOf(FWD), 1)
      const cap = KEY === 'tab' ? 'Tab' : '`'
      eq(
        hintFor(opened.walk),
        `Release Ctrl to switch · ${cap} / Shift+${cap} to move · Esc cancels`,
        'the hint names the held modifier and the walk key',
      )
      const rebound = begin(STACK, openingOf(HELD), 1)
      eq(
        hintFor(rebound.walk),
        `Release Ctrl+Alt to switch · ${cap} / Shift+${cap} to move · Esc cancels`,
        'and follows a rebind to a multi-modifier hold',
      )
    }

    {
      /*
       * A stack read from `localStorage` that names something which is not open any more.
       *
       * The store reconciles both stacks on every snapshot, so this should never happen — and
       * "should never happen" is why `startSwitch` reconciles once more before freezing the
       * order. Without that line the popup draws a row for an id nothing can resolve and the
       * release activates nothing, which is a Ctrl+Tab that visibly did nothing.
       */
      const s = session(['alpha', 'stale', 'beta'], {
        liveAtOpen: ['alpha', 'beta', 'gamma'],
        live: ['alpha', 'beta', 'gamma'],
      })
      s.press(1, FWD)
      eq(s.selected(), 'beta', 'a stale id is reconciled away before the order is frozen')
      s.stroke(FWD)
      eq(s.selected(), 'gamma', 'and the id it had never seen is walkable in its place')
      s.keyup(released)
      eq(s.activated, ['gamma'], 'so the release activates something that exists')
    }

    {
      // An entry closing *during* a walk. The frozen order still names it; the commit must not.
      const s = session(STACK, { live: ['alpha', 'beta', 'delta'] })
      s.press(1, FWD)
      s.stroke(FWD)
      eq(s.selected(), 'gamma', 'the walk still shows the entry that has just closed')
      s.keyup(released)
      eq(s.activated, [], 'releasing on it activates nothing')
      eq(s.order(), ['alpha', 'beta', 'delta'], 'and the stack comes back reconciled')
    }

    {
      // Releasing on an entry that is still there, while another one closed underneath.
      const s = session(STACK, { live: ['alpha', 'beta', 'delta'] })
      s.press(1, FWD)
      s.keyup(released)
      eq(s.activated, ['beta'], 'the surviving selection is activated')
      eq(s.order(), ['beta', 'alpha', 'delta'], 'and the closed entry is gone from the stack')
    }

  }

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

    /*
     * The **tab** stack goes through the same function, and the guard means something else
     * there — which is worth pinning rather than leaving to the comment that says so.
     *
     * `active_tab` is workspace state, so every window's snapshot carries the same answer and
     * there is no "may this window speak" question at all: the shape below is a project with one
     * tab, where the guard fires for the plain reason that a pinned console on its own has
     * nothing to walk.
     */
    const tabs = ['console', 'main.rs', 'lib.rs']
    eq(restack(tabs, ['console', 'main.rs', 'lib.rs'], 'lib.rs'), ['lib.rs', 'console', 'main.rs'],
      'a tab activated anywhere moves to the front of its project’s stack')
    eq(restack(tabs, ['console', 'lib.rs'], 'console'), ['console', 'lib.rs'],
      'and a closed tab leaves it')
    const alone = []
    ok(restack(alone, ['console'], 'console') === alone,
      'a project holding only its pinned console has nothing to walk, so nothing is written')
  }

  /* ------------------------------------------------------------ the store's own wiring */

  /*
   * Everything above is arithmetic and cannot tell you whether anything calls it. These are
   * the claims about `keys/switcherStore.ts` that no pure test can make, each of them standing
   * for a way the gesture fails silently and permanently.
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

    /*
     * 1b. **Exactly one keyup latch in the module, counted rather than asserted to exist.**
     *
     * This is the assertion that makes a *copied* switcher fail the build, and it is the reason
     * the second one was written as a second caller rather than a second store. The gate has one
     * `capture` hook (`keys/useKeyGate.ts` passes `switcherCapture`, singular), so two stores
     * would mean two walks of which only one can ever be consulted — and the loser's `disarm`
     * would never run, leaving a capture-phase `keyup` listener installed for the life of the
     * window and a popup that nothing can close.
     *
     * `includes` cannot see that: a second `armRelease` beside the first passes it. A count can.
     */
    const latches = [...code.matchAll(/addEventListener\('keyup'/g)].length
    eq(latches, 1, 'switcherStore installs exactly one keyup latch, however many switchers there are')
    eq(
      [...code.matchAll(/removeEventListener\('keyup'/g)].length,
      1,
      'and removes exactly that one',
    )

    // 2. The release is a latch, not a third way to resolve a chord. `keys/gate.ts` resolves
    //    on keydown at two entry points that `check-key-gate.mjs` proves agree; a keyup route
    //    into the keymap would be exactly the asymmetry that check exists to catch.
    ok(
      !/from '\.\/keymap'|buildKeymap|\.resolve\(/.test(code),
      'switcherStore resolves no chords — the release watcher is a modifier latch, not an entry point',
    )
    ok(
      code.includes('endsHold(walk.hold, ev)'),
      'and decides purely, through `endsHold`, rather than hard-coding `ev.ctrlKey` — or ' +
        'reverting to `!stillHeld`, which never fires on WebKitGTK',
    )

    // 3. Cancelling writes nothing. The whole point of committing on release.
    const cancel = code.slice(code.indexOf('export function cancelWalk'), code.indexOf('export function commitWalk'))
    ok(cancel.length > 0, 'found cancelWalk')
    ok(
      !cancel.includes('remember') && !cancel.includes('activate') && !cancel.includes('commit('),
      'cancelWalk activates nothing and writes no order — Escape cannot reorder the stack',
    )

    // 4. And committing does both, in that order — through the walk's target, so that the two
    //    switchers cannot end up with two commit paths and one of them missing the reorder.
    const commitBody = code.slice(code.indexOf('export function commitWalk'), code.indexOf('function startSwitch'))
    ok(commitBody.includes('target.remember('), 'commitWalk records the new order')
    ok(commitBody.includes('target.activate('), 'and activates the selection')
    ok(
      commitBody.indexOf('target.remember(') < commitBody.indexOf('target.activate('),
      'order before activation, so a second press inside the round trip reads the new stack',
    )

    /*
     * 5. Both targets are real, and each writes the stack it walks.
     *
     * The abstraction is what stops the two switchers drifting; this is what stops the
     * abstraction being satisfied by a target that remembers nothing. A `remember` that was a
     * no-op would leave every walk starting from the same order for ever — MRU degraded to strip
     * order, silently, with the whole table above still green because the arithmetic is fine.
     */
    for (const [starter, remember, activate] of [
      ['export function startProjectSwitch', 'rememberMru(', 'activateProject('],
      ['export function startTabSwitch', 'rememberTabMru(', 'revealTab('],
    ]) {
      const at = code.indexOf(starter)
      ok(at >= 0, `${starter} exists`)
      const body = code.slice(at, code.indexOf('\n}', at))
      ok(body.includes(remember), `${starter} remembers through ${remember}`)
      ok(body.includes(activate), `and activates through ${activate}`)
      ok(
        body.includes('startSwitch('),
        `${starter} goes through the shared startSwitch — a second copy of the open/advance ` +
          'logic is how two walks come to exist at once',
      )
    }
  }

  {
    /*
     * The hold comes from the *keystroke that dispatched the command*, and nowhere else.
     *
     * This one line is what separates the two behaviours: with a stroke, the chord opens a walk
     * that waits for the release; with `null` it degenerates to an immediate switch — a
     * two-item toggle, which is precisely what the user rejected. `check-key-gate.mjs` proves
     * the gate publishes the stroke; this proves the store still asks for it.
     */
    const store = read('../src/keys/switcherStore.ts')
    ok(
      /begin\([^)]*openingOf\(currentStroke\(\)\)/.test(store),
      'startSwitch reads the opening chord off the dispatching stroke — a constant here would ' +
        'turn the switchers back into the toggle they replaced',
    )
    ok(
      /reconcile\(target\.order\(\)/.test(store),
      'and reconciles the remembered order against the live list before freezing it — the ' +
        'scenario above shows what a walk frozen over a stale id looks like from the outside',
    )
    // And the snapshot rule is the shared one, so the guard on which windows may rewrite the
    // stack cannot be re-inlined without the assertions above noticing. Both stacks go through
    // it: the project one over the header strip, the tab one over each project's tabs.
    const ws = read('../src/store/workspace.ts')
    const wsCode = ws.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
    ok(/restack\(/.test(wsCode), 'the workspace store derives its stacks through `restack`')
    ok(wsCode.includes('function nextMru'), 'nextMru derives its stack from the snapshot')
    ok(
      wsCode.includes(`'cide.projectMru'`),
      'and is persisted under cide.projectMru — a stack that resets every launch makes the ' +
        'first press of every session land on whatever happens to be second, which is the strip ' +
        'order the switchers exist to replace',
    )

    /*
     * The **tab** stack moved into Rust in M15 and its assertion had to move with it, which is
     * the interesting half of this block.
     *
     * It used to be pinned exactly like the project stack: derived here from `project.activeTab`
     * and cached under `localStorage`'s `cide.tabMru`. Both of those assertions would now pass
     * for the wrong reason, and one of them is a trap rather than merely stale — a
     * `cide.tabMru` key still written here would be a *second* order, mirroring a field Rust now
     * owns, and the two would silently disagree the moment a tab was closed from a window that
     * is not this one (`cide_app::ide` closes a withdrawn diff with no webview in the loop at
     * all). So the check is inverted: the key must be **gone**.
     *
     * What replaces it is the same claim one level down. The order is read off the snapshot's
     * `project.tabMru` — `cide_ipc::workspace::Project::tab_mru`, which `close_tab` consults to
     * pick a successor — and the only thing this file may still do to it is `reconcile`, which
     * appends tabs the order has never seen so the switcher can walk to them. `restack` here
     * would be the old derivation coming back: it *touches* the active id, which would re-derive
     * an order that Rust is now the author of.
     */
    ok(wsCode.includes('function nextTabMru'), 'nextTabMru reads the tab order off the snapshot')
    ok(
      !wsCode.includes(`'cide.tabMru'`),
      'and no longer caches one under cide.tabMru — a webview copy of an order Rust owns is a ' +
        'second answer to "which tab do I land on when this one closes"',
    )
    const nextTabMru = /function nextTabMru[\s\S]*?\n\}/.exec(wsCode)?.[0] ?? ''
    ok(
      /project\.tabMru/.test(nextTabMru),
      'nextTabMru reads `project.tabMru`, the field `close_tab` picks its successor from',
    )
    ok(
      /reconcile\(/.test(nextTabMru) && !/restack\(/.test(nextTabMru),
      'and only reconciles it against the live tabs — `restack` would re-derive from ' +
        '`activeTab` the order Rust is now the author of',
    )
    // And the Rust half is actually there. A grep over `cide-ipc` rather than over the generated
    // TypeScript: `generated.ts` is written by `xtask codegen` from these types, so the DTO is
    // the thing that has to carry the field and the binding follows it.
    const projectRs = read('../../crates/cide-ipc/src/workspace.rs').replace(
      /\/\/\/[^\n]*/g,
      '',
    )
    ok(
      /pub tab_mru:\s*Vec<TabId>/.test(projectRs),
      '`Project` carries the tab focus order in Rust, where the close decision is made',
    )
    ok(
      /#\[serde\(default\)\]\s*\n\s*pub tab_mru:/.test(projectRs),
      'and defaults, so every `workspace.json` written before it still deserialises rather ' +
        'than being quarantined with every project in it',
    )
    const closeTab = /pub fn close_tab[\s\S]*?\n\}/.exec(
      read('../../crates/cide-core/src/workspace.rs').replace(/\/\/[^\n]*/g, ''),
    )?.[0]
    ok(
      closeTab !== undefined && /tab_mru/.test(closeTab),
      'and `close_tab` consults it — which is the whole reason it is in Rust and not here',
    )
    ok(
      closeTab !== undefined && /tabs\[\.\.index\]\s*\.iter\(\)\s*\.rev\(\)/.test(closeTab),
      'while keeping the left-neighbour rule as the fallback, for the migrated workspace whose ' +
        'order holds nothing but the tab being closed — as a leftward walk now, because the ' +
        'nearest neighbour can be a tab torn out into its own window and activating one of ' +
        'those would draw it in two windows at once',
    )
  }

  {
    /*
     * The gate has to actually consult the capture, or the popup is decorative and Tab still
     * reaches the shell underneath it.
     *
     * There is **one** `capture` hook and two claims on it since Settings → Keymap grew a
     * keystroke recorder, so this is no longer a bare `capture: switcherCapture` — it is a
     * composition, and what matters here is that the switcher's half is still in it. The
     * recorder is first and short-circuits, which costs the switcher nothing: it is modal, and
     * `startRecording` cancels any open walk before arming. `check-keymap.mjs` pins that order
     * and that cancellation from the other end.
     */
    const wiring = read('../src/keys/useKeyGate.ts')
    ok(
      /capture:[^\n]*switcherCapture\(/.test(wiring) || /capture:\s*switcherCapture\b/.test(wiring),
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
    /*
     * The default bindings, read out of the Rust that defines them.
     *
     * **Three-tuples as well as two**, and that is not a formality: every M14 binding carries a
     * `when` and is therefore invisible to the 2-tuple pattern this block used to have on its
     * own. A check that reads none of the bindings it is asserting about passes vacuously, which
     * is the failure `check-key-gate.mjs` records twice in its own reader.
     */
    const keymapRs = read('../../crates/cide-core/src/keymap.rs')
    const body = keymapRs
      .slice(keymapRs.indexOf('pub fn defaults() -> Vec<Binding> {'))
      .split('\n}')[0]
    const bound = Object.fromEntries([
      ...[...body.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => [m[1], m[2]]),
      ...[...body.matchAll(/\("([^"]+)",\s*"([^"]+)",\s*"([^"]+)"\)/g)].map((m) => [m[1], m[2]]),
    ])
    ok(Object.keys(bound).length >= 20, `read ${Object.keys(bound).length} default bindings`)

    eq(bound['ctrl+tab'], 'tab.switcher.next', 'ctrl+tab is bound to the tab switcher')
    eq(bound['ctrl+shift+tab'], 'tab.switcher.prev', 'and ctrl+shift+tab walks it back')
    /*
     * The project switcher moved to the key left of `1`, and it is bound as **backquote** —
     * written as the literal backtick here, which is what `parse_chord` keeps and what
     * `ui/src/keys/chords.ts` folds `backquote`, `tilde`, `grave` and `~` onto.
     *
     * The literal tilde would have been inert: `strokeFromEvent` reads `code` first and that key
     * is `Backquote` whatever `key` reports, so no keystroke can ever produce the token `~`.
     * Asserting the spelling here is what keeps a "fix" that writes it out as the user asked
     * from shipping a binding nothing can press.
     */
    eq(bound['ctrl+`'], 'project.switcher.next', 'ctrl+` is bound to the project switcher')
    eq(
      bound['ctrl+shift+`'],
      'terminal.splitBelow',
      'and its reverse chord is taken, which is why project.switcher.prev ships unbound',
    )
    ok(
      !Object.keys(bound).some((key) => key.includes('~')),
      'nothing is bound to a literal tilde — no keystroke can produce that token',
    )
  }

  if (failed > 0) {
    console.error(`\ncheck-switcher: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log('check-switcher: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
