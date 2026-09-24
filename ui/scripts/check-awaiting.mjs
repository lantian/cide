/**
 * Checks `src/panes/awaitingRule.ts` — the predicate behind the pane highlight, the tab and
 * project badges, and the `Awaiting: X` in the task bar — and the four surfaces that read it.
 *
 * Worth pinning because the whole feature is one distinction and nothing else in the chain
 * can catch it being wrong. A finished turn is said outright on the wire —
 * `cide_claude::next_state` turns a `Stop` out of `Busy` into `AwaitingInput` — and `Idle`
 * therefore says *nothing* about waiting: it is a fresh prompt at launch, and it is also what
 * `/clear` produces (the fresh conversation's `SessionStart`) on the pane the user is at that
 * moment typing into. If this file regresses towards raising on `Idle`, typing `/clear`
 * notifies the user about their own keystroke; towards clearing on it, a stray `Stop` eats a
 * marker nobody has acknowledged. Either way the user learns to ignore the badge, and the one
 * time it is true they will not look.
 *
 * Three sections:
 *
 * 1. **The rule.** Compiled and driven, the way `check-exit-marker.mjs` and `check-menus.mjs`
 *    do it — there is no JS test runner in this project and this is a pure module the
 *    TypeScript in `node_modules` can compile alone.
 * 2. **The three levels agree.** Pane, tab and project are one function over a pane set, and
 *    the way this feature goes wrong is three flags mutated from three places that half-clear.
 * 3. **The surfaces are wired**, as source assertions. The signal reaching no control is this
 *    project's recurring defect, and it is what had actually happened here: the hook, the
 *    state machine, the report and the OS title were all correct and complete, while
 *    `data-awaiting` sat on an element no stylesheet could reach, no project surface existed
 *    at all, and nothing ever asked for the urgency hint a task bar actually renders. None of
 *    that is visible to a test of the rule, because the rule was never wrong.
 *
 * Run: `pnpm --dir ui run check:awaiting`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-awaiting-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/awaitingRule.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const {
    UNSEEN,
    onState,
    onAcknowledge,
    mergeAuthoritative,
    adopt,
    awaitingAmong,
    awaitingIn,
    isAwaiting,
    awaitingBadge,
    awaitingHint,
    acknowledgesKey,
    acknowledgesButton,
  } = await import(`file://${join(out, 'awaitingRule.js')}`)

  /** Replay a sequence of transitions from nothing. */
  const replay = (...phases) => phases.reduce((track, phase) => onState(track, phase), UNSEEN)

  // --- the distinction the whole feature is ---------------------------------------------
  //
  // A finished turn arrives as `awaitingInput` — Rust decides that on the wire — and `idle`
  // says nothing about waiting in either direction.
  const neverRan = replay('spawning', 'idle')
  const justFinished = replay('spawning', 'idle', 'busy', 'awaitingInput')
  eq(neverRan.awaiting, false, 'a session at a fresh prompt is NOT waiting for the user')
  eq(justFinished.awaiting, true, 'a session that finished a turn IS waiting for the user')

  // `/clear`. The CLI opens a fresh conversation, whose `SessionStart` arrives as `idle` on a
  // session with turns behind it — and the keystrokes that typed the command have already
  // acknowledged. The old rule ("an idle after a busy means finished") re-raised right here,
  // which notified the user about the pane they were typing into.
  eq(
    onState(onAcknowledge(justFinished), 'idle').awaiting,
    false,
    'a conversation restart (/clear) must not announce the pane the user is typing into',
  )
  // ...and the other direction: an `idle` landing on a session already waiting (a `Stop`
  // while the hook map says AwaitingInput) must not eat an unacknowledged marker.
  eq(
    onState(justFinished, 'idle').awaiting,
    true,
    'an `idle` says nothing: it must not clear a marker nobody has acknowledged either',
  )

  // --- the states that decide on their own ----------------------------------------------
  eq(replay('busy').awaiting, false, 'a running turn is not waiting')
  eq(
    replay('spawning', 'idle', 'busy', 'awaitingPermission').awaiting,
    true,
    'a permission prompt is the clearest possible case of waiting',
  )
  eq(
    replay('awaitingPermission').awaiting,
    true,
    'a permission prompt on the very first turn still counts — it does not consult history',
  )
  eq(
    replay('awaitingInput').awaiting,
    true,
    'AwaitingInput counts on the very first transition — it is what a finished turn and a ' +
      'finished shell job both arrive as, and it does not consult history',
  )
  eq(replay('splash').awaiting, false, 'a resume splash is a control already on screen')
  eq(
    replay('spawning', 'idle', 'busy', 'awaitingInput', 'exited').awaiting,
    false,
    'a dead session waits for nobody',
  )
  eq(
    replay('spawning', 'idle', 'busy', 'awaitingInput', 'exited').gone,
    true,
    '`gone` is what tells the caller to drop the entry rather than count a corpse for ever',
  )

  // --- acknowledgement --------------------------------------------------------------------
  const seen = onAcknowledge(justFinished)
  eq(seen.awaiting, false, 'looking at the pane clears the marker')
  eq(
    onState(seen, 'awaitingInput').awaiting,
    true,
    'the NEXT turn ending has to raise it again, or a session announces itself exactly once ' +
      'and then never again',
  )
  eq(
    onState(onState(seen, 'busy'), 'awaitingInput').awaiting,
    true,
    'the ordinary loop — reply, wait, finish — raises the marker every time round',
  )

  // --- which gestures are a person -----------------------------------------------------
  //
  // Driven rather than grepped, because the DOM half of this was grepped and that is exactly
  // how the keyboard acknowledger shipped dead: the assertion below used to read
  // `includes("addEventListener('keydown', onKeyDown)")`, which the broken bubble-phase
  // registration satisfied word for word.
  eq(acknowledgesKey('a'), true, 'an ordinary keystroke in a pane is a person reading it')
  eq(acknowledgesKey('Enter'), true, 'and so is Enter — UP then Enter is the reported gesture')
  eq(acknowledgesKey('ArrowUp'), true, 'and an arrow: recalling the last command is an act')
  eq(
    [acknowledgesKey('Shift'), acknowledgesKey('Control'), acknowledgesKey('Alt'), acknowledgesKey('Meta')],
    [false, false, false, false],
    'bare modifiers are not: a chord is half-typed, or on some layouts a modifier is pressed ' +
      'on the way somewhere else entirely',
  )
  eq(acknowledgesButton(0), true, 'a left click into the pane has always counted')
  eq(
    acknowledgesButton(1),
    true,
    'and the middle one now does: a primary-selection paste puts text into THIS pane',
  )
  eq(
    acknowledgesButton(2),
    false,
    'the right button opens the pane menu, which is where `Minimize window` is chosen from — ' +
      'clearing on the way there destroys the one thing that brings the user back',
  )

  // --- the loop, driven for real ------------------------------------------------------------
  //
  // > "i saw it once only, and after i've focused - nothing more calling me in title, or panel
  // >  - it seems that after first run it failes to work again"
  //
  // The assertions above this line are each one step of that loop. This one drives the whole
  // thing, several times round, because **one turn is the turn that works** — a feature that
  // announces itself exactly once passes every single-turn test there is, and this one shipped
  // with tests that passed. Each round is the complete sequence a real turn produces: the
  // user's keystroke acknowledges, `UserPromptSubmit` makes it busy, tool calls keep it busy,
  // `Stop` ends it — arriving as `awaitingInput`, because Rust decides the finish on the
  // wire. The state after every finish is `awaiting`, and it is the *same* state every time —
  // not merely truthy the first time.
  let track = replay('spawning', 'idle', 'busy', 'awaitingInput') // the first turn just finished
  const step = (turn, at) => ({ turn, at, awaiting: track.awaiting })
  const turns = [step(1, 'finished')]
  for (let turn = 2; turn <= 5; turn++) {
    track = onAcknowledge(track) // the user clicks into the pane
    turns.push(step(turn, 'acknowledged'))
    track = onState(track, 'busy') // UserPromptSubmit: they typed the next thing
    track = onState(track, 'busy') // PreToolUse / PostToolUse: same state, no news
    turns.push(step(turn, 'working'))
    track = onState(track, 'awaitingInput') // Stop out of Busy
    turns.push(step(turn, 'finished'))
  }
  eq(
    turns.filter((t) => t.at === 'finished' && !t.awaiting),
    [],
    'EVERY finished turn raises the marker. A turn that finishes silently after the first is ' +
      'the whole of the reported bug, and it is invisible to any test that drives one turn',
  )
  eq(
    turns.filter((t) => t.at !== 'finished' && t.awaiting),
    [],
    'and nothing else raises it: a marker that never came down would announce a session that ' +
      'is mid-turn, which is worse than announcing nothing',
  )
  eq(
    track,
    onState(
      onState(onAcknowledge(replay('spawning', 'idle', 'busy', 'awaitingInput')), 'busy'),
      'awaitingInput',
    ),
    'the fifth finished turn is in exactly the state the second one was — the loop has no ' +
      'residue, so there is nothing that could wear out',
  )
  // The same loop with Rust's authoritative set in the middle of it, since that is what the
  // running app does: the window reports, Rust aggregates, Rust broadcasts, the window merges
  // the answer back over its own table. A merge that left residue behind would show up as a
  // marker that sticks or a turn that stops announcing, so the loop is driven through it.
  let round = new Map([['s', replay('spawning', 'idle', 'busy', 'awaitingInput')]])
  for (let turn = 1; turn <= 3; turn++) {
    round = mergeAuthoritative(round, ['s']) // Rust echoes the raise back
    round.set('s', onAcknowledge(round.get('s'))) // the user reads the pane
    round = mergeAuthoritative(round, []) // and Rust echoes the clear back
    eq(awaitingIn(round, ['s']), 0, `turn ${turn}: a session that has been read is not counted`)
    round.set('s', onState(round.get('s'), 'busy'))
    round.set('s', onState(round.get('s'), 'awaitingInput'))
    eq(
      awaitingIn(round, ['s']),
      1,
      `turn ${turn}: the next turn ending must raise it again even after a full round trip ` +
        `through Rust's set`,
    )
  }

  // --- the cross-window merge ---------------------------------------------------------------
  //
  // Rust holds the authoritative set and re-broadcasts it, because a window opened after a
  // session started waiting has no history of its own to derive the answer from.
  const local = new Map([
    ['a', onState(UNSEEN, 'busy')],
    ['b', replay('spawning', 'idle', 'busy', 'awaitingInput')],
  ])
  const merged = mergeAuthoritative(local, ['a', 'c'])
  eq(merged.get('a').awaiting, true, 'the broadcast can raise a session this window called busy')
  eq(
    merged.get('b').awaiting,
    false,
    'absence from the set means "no longer waiting" — that is how one window acknowledging ' +
      'clears the marker in the other window showing the same pane',
  )
  eq(
    merged.get('c'),
    { awaiting: true, gone: false },
    'a session this window has never heard of is adopted from the broadcast, which is the ' +
      'entire reason the broadcast exists',
  )
  eq(
    mergeAuthoritative(new Map([['d', onState(UNSEEN, 'exited')]]), []).has('d'),
    false,
    'a dead session is dropped by the merge rather than carried for the life of the window',
  )

  // --- the catch-up a freshly opened window asks for ----------------------------------------
  //
  // `cide://session-awaiting` reports *changes*, so a window built by a detach — which Rust has
  // already titled `Awaiting: 1` — hears nothing and asks instead. The answer describes the set
  // as it was when the question landed, which is why this is a union and the broadcast is not.
  const fresh = adopt(new Map(), ['a'])
  eq(
    fresh.get('a'),
    { awaiting: true, gone: false },
    'a window that has heard nothing takes the whole answer, or a detached pane shows no ' +
      'marker under a title bar that says it is waiting',
  )
  const raced = adopt(new Map([['b', replay('spawning', 'idle', 'busy', 'awaitingInput')]]), [
    'a',
  ])
  eq(
    raced.get('b').awaiting,
    true,
    'the answer was taken before the question landed, so a session that started waiting ' +
      'during the round trip must survive it — `mergeAuthoritative` would clear it, and a ' +
      'finished turn produces no later transition to raise it again',
  )

  // --- the count a background tab shows -----------------------------------------------------
  //
  // A pane in a background tab is mounted but `visibility: hidden`, so its own marker is
  // painted nowhere and is unreachable to pointer and to focus. Hiding a tab is never an
  // unmount — that would destroy the terminals — which is exactly why this can be computed
  // for a tab nobody is looking at.
  // This is the arithmetic that puts the answer on the tab instead, and it is worth pinning
  // for the same reason as the rest of the file: nothing else in the chain can catch it. A
  // count that never reached zero would leave a permanent "come back here" on a tab where
  // everything has been dealt with, and the user would learn to ignore the strip.
  const waiting = () => replay('spawning', 'idle', 'busy', 'awaitingInput')
  const busy = () => onState(UNSEEN, 'busy')

  const table = new Map([
    ['s1', waiting()],
    ['s2', waiting()],
    ['s3', busy()],
  ])

  eq(awaitingIn(table, ['s1']), 1, 'a tab holding one waiting pane counts it')
  eq(awaitingIn(table, ['s1', 's2', 's3']), 2, 'a tab counts each of its waiting sessions')
  eq(awaitingIn(table, ['s3']), 0, 'a tab whose panes are all busy shows nothing')
  eq(awaitingIn(table, []), 0, 'a tab with no panes at all counts nothing')
  eq(
    awaitingIn(table, ['s1', 's1']),
    1,
    'a mirrored pane is two panes showing ONE conversation; counting it twice would be ' +
      'counting windows onto a thing rather than the thing',
  )
  eq(
    awaitingIn(table, [null, undefined, 's1']),
    1,
    'a diff pane, an editor pane and a splash have no session and cannot be waiting',
  )
  eq(
    awaitingIn(table, ['from-a-previous-process']),
    0,
    '`pane.session` survives a restart, so a launched app is full of pane records naming ' +
      'dead children — no news must never read as "come back to this one"',
  )
  eq(
    awaitingIn(new Map([['s1', onState(waiting(), 'exited')]]), ['s1']),
    0,
    'a session that has exited waits for nobody, whatever its pane record still says',
  )

  // The transition that clears it, which is the half a "does it light up" check would miss.
  // Acknowledging is per session — a click or a keystroke into ONE pane — so a tab holding
  // two waiting panes must go 2 → 1 → 0 rather than clearing wholesale. Activating the tab
  // is not one of these steps on purpose: a tab can hold four panes with one waiting, and
  // clearing on activation destroys the only pointer to that one at the moment the user
  // could finally act on it.
  const dealtWithOne = new Map(table).set('s1', onAcknowledge(table.get('s1')))
  eq(
    awaitingIn(dealtWithOne, ['s1', 's2']),
    1,
    'looking at one of two waiting panes decrements the tab rather than clearing it — the ' +
      'other one is still waiting and the tab is the only thing that can still say so',
  )
  const dealtWithBoth = dealtWithOne.set('s2', onAcknowledge(dealtWithOne.get('s2')))
  eq(awaitingIn(dealtWithBoth, ['s1', 's2']), 0, 'the marker clears once the last one is seen')
  eq(
    awaitingIn(
      new Map(dealtWithBoth).set('s1', onState(dealtWithBoth.get('s1'), 'awaitingInput')),
      ['s1', 's2'],
    ),
    1,
    'and the NEXT turn ending raises the tab again, or a tab announces itself exactly once',
  )
  eq(
    awaitingIn(mergeAuthoritative(table, []), ['s1', 's2']),
    0,
    'another window acknowledging both clears this window s tab marker too, through the ' +
      'same broadcast the pane markers follow — one source of truth, so they cannot drift',
  )

  // --- the three levels are one function, and cannot half-clear --------------------------------
  //
  // This is the part the brief warns about and the part nothing else in the chain can catch.
  // There are four attention surfaces — a pane's highlight, its tab's badge, its project's badge
  // in the header, and `Awaiting: N` in the OS title — and the way this feature goes wrong is
  // three booleans mutated from three places, where acknowledging one pane clears its own mark
  // and leaves a tab or a header still saying "come back here" for a pane that has been dealt
  // with. So every surface is a reading of `awaitingAmong` over a different pane set, and these
  // assertions pin that they agree rather than merely that each one works.
  //
  // The window title is the fourth and is computed in Rust (`cmd::window::retitle` over
  // `sessions_of`), for the reason `awaiting.ts` gives: only Rust knows which panes an OS window
  // is showing. It is the same shape — one function over a pane set — and it has its own tests.

  // A project of two tabs. Tab A holds one waiting pane and one busy one; tab B holds a second
  // waiting pane; and a detached pane of the same project mirrors tab A's waiting session —
  // one child, two panes, which is what `claude.mirror` and a torn-out pane both produce.
  const tabA = ['a1', 'a2']
  const tabB = ['b1']
  const torn = ['a1']
  const projectPanes = [...tabA, ...tabB, ...torn]
  const projectTracks = new Map([
    ['a1', waiting()],
    ['a2', busy()],
    ['b1', waiting()],
  ])

  /** Every surface asked about the same panes must give the same answer. */
  const agree = (tracks, panes, what) => {
    const set = awaitingAmong(tracks, panes)
    eq(
      awaitingIn(tracks, panes),
      set.size,
      `${what}: the count a badge shows is the size of the set its panes are marked from`,
    )
    const marked = [...new Set(panes.filter((p) => isAwaiting(tracks, p)))].sort()
    eq(
      marked,
      [...set].sort(),
      `${what}: exactly the panes drawing a highlight are the panes the badge counts — one ` +
        `marked and not counted, or counted and not marked, is the half-clear this module ` +
        `is arranged to make impossible`,
    )
  }

  agree(projectTracks, tabA, 'a tab')
  agree(projectTracks, projectPanes, 'a project')
  eq(
    awaitingIn(projectTracks, projectPanes),
    2,
    'the project counts two conversations, not three panes: the torn-out pane and the docked ' +
      'one are one child, and a header badge saying 3 would be counting windows onto a thing',
  )

  // Acknowledging ONE pane. Its own highlight goes; the other pane's does not; the project's
  // count decrements rather than clearing.
  const oneSeen = new Map(projectTracks).set('a1', onAcknowledge(projectTracks.get('a1')))
  eq(isAwaiting(oneSeen, 'a1'), false, 'the pane that was looked at stops being highlighted')
  eq(
    isAwaiting(oneSeen, 'b1'),
    true,
    'a pane in another tab is untouched — clearing on "some pane was focused" is the bug',
  )
  eq(
    awaitingIn(oneSeen, projectPanes),
    1,
    'and the project still says come back, because one of its panes still wants the user',
  )
  agree(oneSeen, projectPanes, 'a project with one pane dealt with')
  eq(
    awaitingIn(oneSeen, torn),
    0,
    'the mirrored pane clears with the one that was read — they are one conversation, and ' +
      'per-session acknowledgement is what makes that automatic rather than a second rule',
  )

  const allSeen = new Map(oneSeen).set('b1', onAcknowledge(oneSeen.get('b1')))
  eq(
    awaitingIn(allSeen, projectPanes),
    0,
    'the project badge clears only once the LAST of its panes has actually been seen',
  )
  agree(allSeen, projectPanes, 'a project with everything dealt with')
  const againAfter = new Map(allSeen).set('b1', onState(allSeen.get('b1'), 'awaitingInput'))
  eq(
    awaitingIn(againAfter, projectPanes),
    1,
    'and the next turn ending raises the project again, or it announces itself exactly once',
  )
  agree(againAfter, projectPanes, 'a project whose next turn has just ended')

  // A project whose panes hold no session at all — file editors, diffs, resume splashes — plus
  // a pane record naming a child from a previous process. Must be silent, not "unknown means
  // waiting": the header draws a tab for every open project on every launch.
  eq(
    awaitingIn(projectTracks, [null, undefined, 'from-a-previous-process']),
    0,
    'editors, diffs, splashes and pane records naming a dead process contribute nothing to a ' +
      'project badge, or a fresh launch decorates every project tab in the header',
  )

  // --- what the marker actually says ----------------------------------------------------------
  eq(awaitingBadge(0), '', 'nothing waiting draws an empty box, never a zero')
  eq(awaitingBadge(1), '1', 'a count, not a dot: how many decides whether the user goes now')
  eq(awaitingBadge(9), '9', 'nine still fits the reserved box')
  // The box, its slot and every reach-over are one set of numbers, and the empty marker
  // collapses them together.
  //
  // `.awaiting` sets `width: var(--awaiting-w)` and cancels its flex gap from
  // `--awaiting-slot`; `.labelPinned` reaches over the slot in a negative margin and a
  // matching padding; `[data-awaiting='false']` zeroes width and slot as a pair. Written as
  // literals they would drift, and every direction of drift is invisible in the rest state:
  // a width zeroed without its slot is 8px of nothing between a title and its close button, a
  // slot zeroed without the reach following is a pinned label poking past its own tab's edge,
  // and no collapse at all is the permanent 26px reserve this design replaced. Asserted on
  // the stylesheet because there is no DOM here.
  {
    const css = readFileSync(new URL('../src/chrome/TabStrip.module.css', import.meta.url), 'utf8')
    eq(
      /width:\s*var\(--awaiting-w\)/.test(css),
      true,
      'the chip takes its width from --awaiting-w rather than a literal',
    )
    eq(
      (css.match(/var\(--awaiting-w\)/g) ?? []).length,
      3,
      'exactly three readers of the width — the slot definition, the chip, and its ' +
        'gap-cancel — one number',
    )
    eq(
      (css.match(/var\(--awaiting-slot\)/g) ?? []).length,
      3,
      "and three of the slot — both halves of `.labelPinned`'s reach-over and the chip's " +
        'gap-cancel — or the label reach drifts from the geometry it reaches over',
    )
    eq(
      /\.tab\[data-awaiting='false'\]\s*\{\s*--awaiting-w:\s*0px;\s*--awaiting-slot:\s*0px;\s*\}/.test(
        css,
      ),
      true,
      'an empty marker collapses BOTH numbers together — the width alone leaves the flex ' +
        'gap, the slot alone leaves the label reach; either half is invisible until hovered',
    )

    /*
     * The box is big enough for the glyphs it has to hold.
     *
     * This is the assertion the reported bug needed and did not have. The chip was 13px square
     * with a 9.5px digit; the ink filled it corner to corner and it was reported as "missing
     * padding" — which it was, except that a fixed reserve has no padding property to add, so
     * the number to change is the reserve itself. Every other assertion here passed throughout,
     * because they check that the three declarations *agree*, not that the number is usable.
     *
     * `9+` is the widest label `awaitingBadge` can produce (asserted just below), and the mono
     * family advances 0.6em per character. So the smallest box that fits the label is
     * `2 * 0.6 * fontSize`, and anything at or under that is the corner-to-corner state again.
     * The 3px floor is what makes it read as a badge rather than as a filled rectangle: it is
     * the gap the reporter could see was missing.
     */
    /*
     * All four numbers are read at the *design* chrome size, and the geometry is asserted
     * there rather than at every size in the band.
     *
     * That is sound rather than a shortcut, and only because all four scale by the same
     * `--ui-scale`: the reserve, the height, the line-height and the glyph. `slack = box −
     * 1.2 × fontSize` is linear in the scale with no constant term, so a ratio that clears 3px
     * at 13 clears `3 × scale` at any other size and can never cross zero. The one way to
     * break that is to leave one of the four unscaled while scaling the others — which is
     * exactly what the reserve was, and what the `calc()`-aware patterns below now catch,
     * because an unscaled literal no longer matches them at all.
     */
    const px = (name, pattern) => {
      const hit = pattern.exec(css)
      if (hit === null) throw new Error(`check:awaiting could not read ${name} from the stylesheet`)
      return Number(hit[1])
    }
    const reserve = px('--awaiting-w', /--awaiting-w:\s*calc\(([\d.]+)px \* var\(--ui-scale\)\)/)
    // The ladder spells a design size with a dash for the decimal point — `--fs-ui-11-5` is
    // 11.5px at the default — so the name has to be turned back into a number rather than
    // read as one. `Number('11-5')` is `NaN`, and a `NaN` here makes every comparison below
    // false, which fails this check with a message about slack rather than about parsing.
    const fontSize = Number(
      (/\.awaiting\b[^}]*?font-size:\s*var\(--fs-ui-([\d-]+)\)/s.exec(css)?.[1] ?? '').replace(
        '-',
        '.',
      ),
    )
    if (!Number.isFinite(fontSize)) {
      throw new Error('check:awaiting could not read .awaiting font-size from the stylesheet')
    }
    const height = px(
      '.awaiting height',
      /\.awaiting\b[^}]*?height:\s*calc\(([\d.]+)px \* var\(--ui-scale\)\)/s,
    )
    const label = 2 * 0.6 * fontSize

    eq(
      reserve - label >= 3,
      true,
      `the chip must leave room around its widest label: ${reserve}px box, ` +
        `${label.toFixed(1)}px of glyph at ${fontSize}px mono — under 3px of total slack is ` +
        'the corner-to-corner state that was reported as missing padding',
    )
    eq(
      height - fontSize >= 3,
      true,
      `and vertically too: ${height}px box against a ${fontSize}px glyph`,
    )
    eq(
      /\.awaiting\b[^}]*?line-height:\s*calc\(([\d.]+)px \* var\(--ui-scale\)\)/s.exec(css)?.[1],
      String(height),
      'line-height matches the height, or the digit is not centred in the box it was given',
    )
  }

  eq(
    awaitingBadge(10),
    '9+',
    'capped, because the box is fixed width — a marker that widened its tab would shove ' +
      'every tab after it sideways under the pointer, which is the one thing it may not do',
  )
  eq(awaitingHint(0, 'tab'), undefined, 'no sentence when there is nothing to say')
  eq(
    awaitingHint(1, 'tab'),
    '1 session in this tab is waiting for you',
    'singular, because "1 sessions" is exactly the kind of thing that ships',
  )
  eq(awaitingHint(3, 'tab'), '3 sessions in this tab are waiting for you', 'plural')
  eq(
    awaitingHint(11, 'tab'),
    '11 sessions in this tab are waiting for you',
    'the tooltip is uncapped — it is the only place a user learns that `9+` means eleven',
  )
  eq(
    awaitingHint(2, 'project'),
    '2 sessions in this project are waiting for you',
    'the container is a parameter so the header s project tabs need no second copy of the ' +
      'wording when they adopt this',
  )

  // --- the surfaces are actually wired ---------------------------------------------------------
  //
  // Source assertions, because this is the half that was broken and it is invisible to every
  // test of the rule: the rule was right the whole time. Each of these is a link that existed,
  // was correct, and reached nothing a user could see.

  const src = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8')
  const has = (path, needle, what) => {
    if (!src(path).includes(needle)) {
      console.error(`FAIL ${what}\n  ${path} does not contain: ${needle}`)
      failed++
    }
  }
  const lacks = (path, needle, what) => {
    if (src(path).includes(needle)) {
      console.error(`FAIL ${what}\n  ${path} still contains: ${needle}`)
      failed++
    }
  }

  // 1. The pane. `data-awaiting` used to be set on the floating control cluster, which is a
  //    child of the frame — so the panel itself could not be selected on and the entire
  //    on-screen signal for a waiting pane was a 7px dot in its corner. Both halves are pinned:
  //    the attribute on the frame, and a rule that reads it.
  const frameTag = src('src/layout/PaneTitleBar.tsx').split('data-audit="pane"')[1] ?? ''
  if (!frameTag.slice(0, 2000).includes('data-awaiting=')) {
    console.error(
      'FAIL the pane frame carries data-awaiting\n' +
        '  PaneTitleBar.tsx sets it somewhere else — on the cluster, it highlights nothing',
    )
    failed++
  }
  has(
    'src/layout/PaneTitleBar.module.css',
    "[data-awaiting='true']",
    'the stylesheet highlights the waiting pane; without a rule the attribute is decoration',
  )

  // 2. The project tab — the surface that did not exist at all. `App.tsx` renders tab strips
  //    and pane trees for the ACTIVE project only, so for every other project both the pane
  //    marker and the tab badge are painted nowhere while Rust counts them into the OS title.
  // The *call*, not the name: this file explains at length why the tab is its own component
  // and names the hook while doing so, so a bare `includes('useAwaitingInProject')` is
  // satisfied by the prose alone and passes with the call deleted. Checked by mutation.
  has(
    'src/chrome/AppHeader.tsx',
    '= useAwaitingInProject(',
    'the header asks how many of each project s sessions are waiting — this is the only ' +
      'surface a background project has, and it had none',
  )
  has(
    'src/chrome/AppHeader.tsx',
    'awaitingBadge(',
    'and draws the count, not a dot: a dot cannot decrement, and decrementing is the only ' +
      'progress a user gets while those panes are not rendered',
  )
  has(
    'src/chrome/AppHeader.module.css',
    '.awaitingOn',
    'the badge is styled; an unstyled span is an invisible badge',
  )

  // 3. Clearing is an act by a *person*. `term.onData` is xterm's outbound stream and the
  //    terminal answers Device Attributes, cursor position and focus reports on it by itself —
  //    so acknowledging there let the CLI's own end-of-turn probe clear the marker with nobody
  //    at the keyboard, in exactly the situation the feature exists for.
  //
  //    The handler lives in `sessionSink.ts` now — keystrokes ride the sink, whose lifetime
  //    is the host's claim on the session rather than a mount — so the slice is taken there,
  //    and its presence is asserted first: the original form indexed into TerminalPane.tsx
  //    and passed vacuously with the handler deleted.
  const sink = src('src/panes/sessionSink.ts')
  const onDataAt = sink.indexOf('term.onData(')
  if (onDataAt === -1) {
    console.error(
      'FAIL sessionSink.ts registers no term.onData handler\n' +
        '  the keystroke path lives with the sink; without it nothing reaches the child',
    )
    failed++
  } else {
    const onData = sink.slice(onDataAt)
    if (onData.slice(0, onData.indexOf('})')).includes('acknowledge(')) {
      console.error(
        'FAIL the awaiting marker is not cleared by terminal output\n' +
          '  sessionSink.ts acknowledges inside term.onData, which fires for the terminal s ' +
          'own automatic replies — the CLI probing the terminal after a turn would clear it',
      )
      failed++
    }
  }
  lacks(
    'src/panes/TerminalPane.tsx',
    'term.onData(',
    'the keystroke handler is the sink s alone — a second one per mount would send every ' +
      'keystroke twice',
  )

  //    ...and it acknowledges in the **capture** phase, which is the assertion this file did
  //    not have and needed. The previous form asked only that a `keydown` listener existed,
  //    and the listener that existed was registered on the bubble — where it is silently DEAD:
  //    xterm's handlers sit on `term.textarea`, a descendant of the element `term.open()` was
  //    given, and its `_keyDown` ends every key it consumes with `cancel(ev, true)`, which is
  //    `preventDefault()` and `stopPropagation()`. So the listener fired for the keys xterm
  //    ignores and never for a printable character, `Enter` or an arrow — the whole of the
  //    report that the marker could only be cleared by clicking a pane the user was already
  //    typing in. Nothing else in the chain can catch this: the rule was right, the surfaces
  //    were wired, and the one dead link passed a check that spelled it out by name.
  has(
    'src/panes/TerminalPane.tsx',
    "addEventListener('keydown', onKeyDown, true)",
    'a real keystroke in the pane acknowledges, and in the CAPTURE phase — on the bubble the ' +
      'listener never sees a printable key, Enter or an arrow, because xterm stops the event ' +
      'at the textarea',
  )
  has(
    'src/panes/TerminalPane.tsx',
    "removeEventListener('keydown', onKeyDown, true)",
    'and comes off with the same flag: removal matches on it, and the host outlives the ' +
      'mount, so a bare removal leaks one acknowledger per mount onto a live terminal',
  )
  //    Scrolling the scrollback is reading it, and it is the only such act with no keyboard in
  //    it at all — which is how a finished `make` is usually read.
  has(
    'src/panes/TerminalPane.tsx',
    "addEventListener('wheel', onWheel, { capture: true, passive: true })",
    'scrolling a waiting pane acknowledges it too — capture for the reason above, passive ' +
      'because a non-passive wheel listener makes the browser wait on it before scrolling',
  )
  has(
    'src/panes/TerminalPane.tsx',
    "removeEventListener('wheel', onWheel, true)",
    'and comes off with the capture flag, for the same reason as the keystroke one',
  )
  //    The pointer screen is the rule module's, not a literal. Written out at the listener it
  //    was `button === 0`, which is a statement about a number rather than about people, and
  //    the middle button — a primary-selection paste straight into the pane — fell through it.
  has(
    'src/layout/PaneTitleBar.tsx',
    'acknowledgesButton(event.button)',
    'the pane frame asks the rule module which buttons are a person, so the answer cannot ' +
      'drift from the one this file drives above',
  )

  // 4. The task bar. The title was already being set correctly and the report was still "no
  //    cide title change in taskbar" — because a task bar is not obliged to draw a title
  //    (Plasma's default Icons-only Task Manager puts it in a tooltip). The urgency hint is
  //    what it does draw, and nothing in the tree had ever asked for it.
  const rust = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8')
  const retitle = rust('crates/cide-app/src/cmd/window.rs')
  const body = retitle.slice(retitle.indexOf('pub(crate) fn retitle'))
  // Both surfaces go through `apply_announcement`, which is the one place allowed to skip
  // the GTK calls when a window already wears the computed answer — the decision stays a
  // level, only the redundant syscall is elided. Pinned in two halves: retitle routes
  // through the applier, and the applier still touches both surfaces.
  if (!body.slice(0, body.indexOf('\n}\n')).includes('apply_announcement')) {
    console.error(
      'FAIL retitle raises and lowers the urgency hint with the title\n' +
        '  cmd/window.rs no longer routes through `windows::apply_announcement`, which is ' +
        'the one call that moves both surfaces from one decision',
    )
    failed++
  }
  const windowsRs = rust('crates/cide-app/src/windows.rs')
  const applier = windowsRs.slice(windowsRs.indexOf('pub fn apply_announcement'))
  const applierBody = applier.slice(0, applier.indexOf('\n}\n'))
  if (!applierBody.includes('set_title(') || !applierBody.includes('demand_attention(')) {
    console.error(
      'FAIL apply_announcement moves the title AND the urgency hint together\n' +
        '  windows.rs applies one surface without the other, which is how a window ends up ' +
        'flashing with `Awaiting: 0` in its title — or titled `Awaiting: 1` in a task bar ' +
        'that never renders titles',
    )
    failed++
  }
  // 4b. A shell pane raises the same signal. Every surface above keys off a *session id* and
  //     never on the pane's kind, so the only reason a bash pane stayed dark was that a shell
  //     emitted exactly one `SessionState` in its life — `Exited`, at death. Both halves of
  //     what changed that are pinned here, because both fail silently: a shell that stops
  //     being watched simply never notifies again, and a Claude pane that starts being
  //     watched gets a second, contradictory opinion about its own state on every tool call.
  const spawn = rust('crates/cide-app/src/cmd/session.rs')
  // `is_console` since M93: a codex console reports through hooks exactly as a claude one does,
  //     so neither may be watched — the gate is "any console", not "claude".
  if (!/if !is_console \{\s*\n\s*spec = spec\.watch_jobs\(/.test(spawn)) {
    console.error(
      'FAIL a shell pane watches its foreground process group, and a Claude pane does not\n' +
        '  cmd/session.rs no longer gates `watch_jobs` on `!is_console` — either a finished ' +
        '`make` notifies nobody, or the pgid watcher is fighting the hooks for one SessionState',
    )
    failed++
  }
  const lifecycle = rust('crates/cide-app/src/lifecycle.rs')
  const mapping = lifecycle.slice(lifecycle.indexOf('fn job_state('))
  const arms = mapping.slice(0, mapping.indexOf('\n}\n'))
  if (!/JobEvent::Finished \{ \.\. \} => SessionState::AwaitingInput/.test(arms)) {
    console.error(
      'FAIL a finished shell job asks for the user rather than merely going idle\n' +
        '  lifecycle.rs maps it to something else — `Idle` says nothing about waiting at ' +
        'all (`awaitingRule.ts` neither raises nor clears on it), so a finished job mapped ' +
        'to it would light no surface anywhere',
    )
    failed++
  }

  // 5. …and the hint is a LEVEL, not an edge. This is the fix for "i saw it once only, and
  //    after i've focused - nothing more calling me in title, or panel". An urgency hint on
  //    the *active* window is dropped rather than deferred (KWin's `demandAttention` opens
  //    `if (isActive()) set = false;`), and the toolkit caches the flag, so a raise made while
  //    the user sits in the window is thrown away AND makes the next raise a no-op. The old
  //    code asked exactly once, at the instant the waiting count moved, and handled only
  //    `Focused(true)` — so every turn after the first, which is every turn that finishes
  //    while the user is in the window, was announced to nobody and never re-announced when
  //    they walked away. Both halves are pinned: the decision knows about focus, and both
  //    directions of the focus event reach the recompute.
  const lib = rust('crates/cide-app/src/lib.rs')
  if (!body.slice(0, body.indexOf('\n}\n')).includes('windows::announce(')) {
    console.error(
      'FAIL the urgency hint is decided by `windows::announce`, which knows about focus\n' +
        '  cmd/window.rs computes it some other way. A bare `count > 0` asks a focused window ' +
        'to flash, the window manager drops the request, and nothing ever asks again',
    )
    failed++
  }
  if (lib.includes('WindowEvent::Focused(true) =>') || !lib.includes('focus_changed(')) {
    console.error(
      'FAIL both directions of WindowEvent::Focused recompute what a window announces\n' +
        '  lib.rs handles only the focus ARRIVING. Losing it is the moment a window holding ' +
        'an unread finished turn has to start calling the user, and it is the only moment ' +
        'left: a session that is already waiting never moves the count again',
    )
    failed++
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('awaiting rule: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
