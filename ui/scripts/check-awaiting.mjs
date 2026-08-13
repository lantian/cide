/**
 * Checks `src/panes/awaitingRule.ts` — the predicate behind the pane highlight, the tab and
 * project badges, and the `Awaiting: X` in the task bar — and the four surfaces that read it.
 *
 * Worth pinning because the whole feature is this one distinction and nothing else in the
 * chain can catch it being wrong. `SessionState::Idle` is two different situations wearing one
 * name: a session that started and has never been asked anything, and a session that has just
 * finished a turn. Rust cannot tell them apart from a single event — that is why the rule is
 * here and not in `hooks.rs` — so if this file regresses, every launch announces "3 sessions
 * are waiting for you" about three sessions that have never run, the user learns to ignore the
 * badge, and the one time it is true they will not look.
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
  } = await import(`file://${join(out, 'awaitingRule.js')}`)

  /** Replay a sequence of transitions from nothing. */
  const replay = (...phases) => phases.reduce((track, phase) => onState(track, phase), UNSEEN)

  // --- the distinction the whole feature is ---------------------------------------------
  //
  // Both of these end in `idle`. If they ever agree, the badge is noise.
  const neverRan = replay('spawning', 'idle')
  const justFinished = replay('spawning', 'idle', 'busy', 'idle')
  eq(neverRan.awaiting, false, 'a session at a fresh prompt is NOT waiting for the user')
  eq(justFinished.awaiting, true, 'a session that finished a turn IS waiting for the user')
  eq(
    neverRan.awaiting === justFinished.awaiting,
    false,
    'both paths end in `idle`; if they agree, every launch announces sessions nobody started',
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
    'AwaitingInput is declared in `SessionState` and never yet produced; when something ' +
      'produces it, it must already count rather than needing this file changed again',
  )
  eq(replay('splash').awaiting, false, 'a resume splash is a control already on screen')
  eq(
    replay('spawning', 'idle', 'busy', 'idle', 'exited').awaiting,
    false,
    'a dead session waits for nobody',
  )
  eq(
    replay('spawning', 'idle', 'busy', 'idle', 'exited').gone,
    true,
    '`gone` is what tells the caller to drop the entry rather than count a corpse for ever',
  )

  // --- acknowledgement --------------------------------------------------------------------
  const seen = onAcknowledge(justFinished)
  eq(seen.awaiting, false, 'looking at the pane clears the marker')
  eq(
    onState(seen, 'idle').awaiting,
    true,
    'acknowledging must not clear `ranATurn`: the NEXT turn ending has to raise it again, ' +
      'or a session announces itself exactly once and then never again',
  )
  eq(
    onState(onState(seen, 'busy'), 'idle').awaiting,
    true,
    'the ordinary loop — reply, wait, finish — raises the marker every time round',
  )

  // --- the cross-window merge ---------------------------------------------------------------
  //
  // Rust holds the authoritative set and re-broadcasts it, because a window opened after a
  // session started waiting has no history of its own to derive the answer from.
  const local = new Map([
    ['a', onState(UNSEEN, 'busy')],
    ['b', replay('spawning', 'idle', 'busy', 'idle')],
  ])
  const merged = mergeAuthoritative(local, ['a', 'c'])
  eq(merged.get('a').awaiting, true, 'the broadcast can raise a session this window called busy')
  eq(
    merged.get('b').awaiting,
    false,
    'absence from the set means "no longer waiting" — that is how one window acknowledging ' +
      'clears the marker in the other window showing the same pane',
  )
  eq(merged.get('a').ranATurn, true, 'per-window history survives the merge')
  eq(
    merged.get('c'),
    { ranATurn: true, awaiting: true, gone: false },
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
    { ranATurn: true, awaiting: true, gone: false },
    'a window that has heard nothing takes the whole answer, or a detached pane shows no ' +
      'marker under a title bar that says it is waiting',
  )
  const raced = adopt(new Map([['b', replay('spawning', 'idle', 'busy', 'idle')]]), ['a'])
  eq(
    raced.get('b').awaiting,
    true,
    'the answer was taken before the question landed, so a session that started waiting ' +
      'during the round trip must survive it — `mergeAuthoritative` would clear it, and a ' +
      'finished turn produces no later transition to raise it again',
  )
  eq(
    adopt(new Map([['a', onState(UNSEEN, 'busy')]]), ['a']).get('a').ranATurn,
    true,
    'per-window history survives the catch-up too',
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
  const waiting = () => replay('spawning', 'idle', 'busy', 'idle')
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
    awaitingIn(new Map(dealtWithBoth).set('s1', onState(dealtWithBoth.get('s1'), 'idle')), [
      's1',
      's2',
    ]),
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
  const againAfter = new Map(allSeen).set('b1', onState(allSeen.get('b1'), 'idle'))
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
  eq(awaitingBadge(9), '9', 'nine still fits the 13px box')
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
  const terminal = src('src/panes/TerminalPane.tsx')
  const onData = terminal.slice(terminal.indexOf('term.onData('))
  if (onData.slice(0, onData.indexOf('})')).includes('acknowledge(')) {
    console.error(
      'FAIL the awaiting marker is not cleared by terminal output\n' +
        '  TerminalPane.tsx acknowledges inside term.onData, which fires for the terminal s ' +
        'own automatic replies — the CLI probing the terminal after a turn would clear it',
    )
    failed++
  }
  has(
    'src/panes/TerminalPane.tsx',
    "addEventListener('keydown', onKeyDown)",
    'a real keystroke in the pane is what acknowledges instead',
  )

  // 4. The task bar. The title was already being set correctly and the report was still "no
  //    cide title change in taskbar" — because a task bar is not obliged to draw a title
  //    (Plasma's default Icons-only Task Manager puts it in a tooltip). The urgency hint is
  //    what it does draw, and nothing in the tree had ever asked for it.
  const rust = (path) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8')
  const retitle = rust('crates/cide-app/src/cmd/window.rs')
  const body = retitle.slice(retitle.indexOf('pub(crate) fn retitle'))
  if (!body.slice(0, body.indexOf('\n}\n')).includes('demand_attention')) {
    console.error(
      'FAIL retitle raises and lowers the urgency hint with the title\n' +
        '  cmd/window.rs sets the title alone, which a task bar need not render at all',
    )
    failed++
  }
  if (!rust('crates/cide-app/src/lib.rs').includes('WindowEvent::Focused(true)')) {
    console.error(
      'FAIL focusing a window clears its urgency hint\n' +
        '  lib.rs has no Focused(true) arm. Tauri documents that a window manager MIGHT NOT ' +
        'clear the hint on input, so a task entry lit once stays lit for the life of the run',
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
