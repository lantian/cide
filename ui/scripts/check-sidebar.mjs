/**
 * Checks the two import-free modules the left sidebar's behaviour lives in — `sidebarWidth.ts`
 * (how wide it is) and `sidebarView.ts` (whether it is showing at all, and which panel) — and
 * pins the width constants against the three other files that hold the same numbers.
 *
 * This exists because both halves of the feature fail *silently*. A clamp that lets a width
 * through gives a panel that swallows the workspace, with no error anywhere; a persistence
 * round trip that drops a field gives a panel that quietly returns to 252px on every launch,
 * which looks exactly like a feature that was never built. There is no JS test runner in this
 * project and the app must never be launched to look at a layout, so the proof has to be the
 * pure logic plus the files it is supposed to agree with.
 *
 * Same shape as `check-theme.mjs`: `sidebarWidth.ts` is import-free on purpose, so the
 * TypeScript in `node_modules` can compile it on its own.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a `pointermove` moves the edge. There is no DOM in this process. That
 *     `setProperty('--w-sidebar-files', …)` on `<html>` resizes the panel rests on the four
 *     panel stylesheets saying `width: var(--w-sidebar-…)`, which this file *does* check,
 *     and on the cascade, which it cannot.
 *   - that the drag avoids a React re-render. That is a property of the code path — nothing
 *     between `pointerdown` and `pointerup` calls into the store — and is argued in the
 *     comment at the top of `SidebarSplitter.tsx`, not measured here.
 *   - that `localStorage` survives a restart in a WebKitGTK webview. If it does not, the
 *     width still restores from `workspace.json`, one frame later; that fallback is the
 *     reason the cache is allowed to be a hint.
 *   - the round trip through Rust. `crates/cide-ipc/src/settings.rs` owns that, and its
 *     tests assert on the camelCase wire names this file's `StoredSidebar` mirrors.
 *
 * Run: `pnpm --dir ui run check:sidebar`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-sidebar-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/sidebarWidth.ts',
      // Compiled beside it and for the same reason: F4's "which panel comes back" is a rule
      // with cases in it, and a rule that lives in a React state updater is a rule no check
      // script can run. Both files are import-free precisely so this one `tsc` can take them.
      'src/chrome/sidebarView.ts',
      // The third, and the same argument once more. `panelRequests.ts` is how anything outside
      // React reveals a panel, and its rules are a TTL, a claimed-once request and a refusal to
      // overwrite a commit message somebody is halfway through — the last of which is the only
      // thing in this flow with no undo. It imports one *type* from `sidebarView.ts`, which is
      // compiled in this same invocation and elided from the output, so node can still load the
      // emitted JavaScript with no resolver.
      'src/chrome/panelRequests.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    SIDEBAR_TOKEN,
    SIDEBAR_DEFAULT,
    SIDEBAR_MIN,
    SIDEBAR_MAX,
    SIDEBAR_CACHE_KEY,
    RAIL_WIDTH,
    SPLITTER_WIDTH,
    MIN_WORKSPACE,
    sidebarCeiling,
    clampSidebarWidth,
    widthFromDrag,
    withPanel,
    widthsFromSettings,
    toStored,
    widthDeclarations,
    encodeWidths,
    decodeWidths,
  } = await import(`file://${join(out, 'sidebarWidth.js')}`)

  const { SIDEBAR_INITIAL, isPanelOpen, selectView, showPanel, toggleSidebar } = await import(
    `file://${join(out, 'sidebarView.js')}`
  )

  // --- hiding the panel, and what comes back ---------------------------------------------
  //
  // The hidden state has shipped since M3 and was reachable by mouse only; F4 is what reaches
  // it from the keyboard, and `last` — which panel comes back — is the only genuinely new fact.
  // Both halves are decided by these four functions and by nothing in `App.tsx`.

  eq(SIDEBAR_INITIAL, { view: 'files', last: 'files' }, 'a shell window opens on Files, showing')
  ok(isPanelOpen(SIDEBAR_INITIAL), 'and that counts as open')

  // The rail: the lit button toggles its own panel shut, any other switches to it.
  eq(selectView(SIDEBAR_INITIAL, 'files'), { view: null, last: 'files' }, 'the lit button hides')
  eq(selectView(SIDEBAR_INITIAL, 'git'), { view: 'git', last: 'git' }, 'another switches')
  eq(
    selectView({ view: null, last: 'git' }, 'search'),
    { view: 'search', last: 'search' },
    'and a click while hidden opens the one clicked, not the one remembered',
  )

  // A command that names a panel always reveals it — never toggles. `keys/dispatch.ts` argues
  // it under `sidebar.search`: the rail button is right there, and a second Ctrl+Shift+F that
  // closed the panel would take away the one thing the binding is for.
  eq(
    showPanel({ view: 'search', last: 'search' }, 'search'),
    { view: 'search', last: 'search' },
    'showPanel on the panel already showing leaves it showing',
  )
  eq(showPanel({ view: null, last: 'git' }, 'files'), { view: 'files', last: 'files' })

  // F4 itself.
  eq(toggleSidebar(SIDEBAR_INITIAL), { view: null, last: 'files' }, 'F4 hides the open panel')
  eq(
    toggleSidebar({ view: null, last: 'problems' }),
    { view: 'problems', last: 'problems' },
    'and brings back the last one that was open, not a hard-coded Files',
  )
  eq(
    toggleSidebar(toggleSidebar({ view: 'git', last: 'git' })),
    { view: 'git', last: 'git' },
    'so F4 twice is a round trip',
  )

  /*
   * `settings` is a view with no panel, and it counts as **closed**. Two assertions, because
   * both halves are easy to get wrong and both are visible to the user:
   *
   *  - F4 with ⚙ lit must open `last` rather than doing nothing, or the key looks broken
   *    exactly once per session — right after somebody clicks the gear;
   *  - `last` must never become `settings`, or "bring back the last panel" brings back a state
   *    with nothing in it and F4 stops being a toggle at all.
   */
  eq(
    selectView({ view: 'files', last: 'files' }, 'settings'),
    { view: 'files', last: 'files' },
    '⚙ changes the sidebar not at all: it opens a workspace TAB, so the panel that was showing '
      + 'keeps showing and no rail button lights up for it. It used to return `view: settings`, '
      + 'which left the gear lit with nothing under it until another button was pressed — '
      + 'reported as "it stucks in active state"',
  )
  eq(
    selectView({ view: null, last: 'git' }, 'settings'),
    { view: null, last: 'git' },
    '…and a hidden sidebar stays hidden, rather than the gear making something appear',
  )
  ok(
    selectView({ view: 'files', last: 'files' }, 'settings').view !== 'settings',
    '…so the state that produced the stuck button is not reachable from a rail click. The type '
      + 'says so too — `SidebarState.view` is a `PanelView` — and this is the runtime half',
  )
  for (const from of [
    { view: 'files', last: 'files' },
    { view: null, last: 'search' },
    { view: null, last: 'problems' },
  ]) {
    let start = from
    for (const step of [
      (s) => selectView(s, 'settings'),
      (s) => selectView(s, 'git'),
      (s) => selectView(s, 'git'),
      (s) => toggleSidebar(s),
      (s) => showPanel(s, 'problems'),
      (s) => toggleSidebar(s),
      (s) => selectView(s, 'settings'),
      (s) => toggleSidebar(s),
    ]) {
      start = step(start)
      ok(start.last !== 'settings', `\`last\` is never the panel-less view: ${JSON.stringify(start)}`)
      ok(start.last !== null, '…and is never null, so there is always something to restore')
    }
    ok(isPanelOpen(start), 'a sequence ending in a toggle from settings leaves a panel showing')
  }

  ok(!isPanelOpen({ view: null, last: 'files' }), 'and a hidden sidebar is not open')

  // --- reaching a panel from outside React -------------------------------------------------
  //
  // `chrome/panelRequests.ts`. Revealing a panel used to be a `useState` setter threaded into
  // `keys/dispatch.ts` as a prop, which meant that anything that was neither the dispatcher nor a
  // descendant of `App` could not reveal one at all — and the git log's *Amend…*, whose entire
  // job is to put the user in front of the commit box, shipped **listed and disabled** for
  // exactly that reason. Everything below is what replaced it.
  //
  // Every assertion here is a rule that would otherwise live inside a React effect, which is the
  // one place in this codebase no check script can reach. Three of them describe things that go
  // wrong *silently*: a request nobody drains, a request that fires minutes after the click it
  // answers, and a request that throws away a commit message the user was writing.

  const pr = await import(`file://${join(out, 'panelRequests.js')}`)

  {
    pr.__resetPanelRequests()
    const seen = []

    // 1. No host: a detached-pane window has no sidebar and never will. The refusal is the whole
    //    value of the slot being fillable — without it the caller parks a request nobody can
    //    answer and reports success, which is the silent no-op the seam exists to remove.
    ok(!pr.panelHostPresent(), 'a window that registered nothing has no panel host')
    eq(pr.requestPanel('git'), false, 'and `requestPanel` refuses rather than parking')
    eq(
      pr.requestAmend({ project: 'p', repo: 'r', oid: 'a'.repeat(40), shortOid: 'aaaaaaa', message: 'm' }),
      false,
      'and so does an amend — nothing is parked for a panel that cannot exist',
    )
    ok(!pr.amendPending(), 'nothing was parked')

    // 2. With a host, the request lands, once, with the view it named.
    pr.registerPanelHost((view) => seen.push(view))
    ok(pr.panelHostPresent(), 'registering fills the slot')
    eq(pr.requestPanel('search'), true, 'and the request lands')
    eq(seen, ['search'], 'with the view it named, exactly once')
    eq(pr.requestPanel('git'), true)
    eq(seen, ['search', 'git'], 'a second request reveals again — this is not a toggle')

    // 3. Unregistering is not optional: a root that went away must stop receiving.
    pr.registerPanelHost(null)
    eq(pr.requestPanel('files'), false, 'unregistering empties the slot')
    eq(seen, ['search', 'git'], 'and a dead host is not called')
  }

  {
    // --- the amend request: parked, revealed, claimed once -----------------------------------
    pr.__resetPanelRequests()
    const seen = []
    let woke = 0
    pr.registerPanelHost((view) => seen.push(view))
    const stop = pr.subscribeAmendRequest(() => {
      woke++
    })

    const request = {
      project: 'proj-1',
      repo: 'repo-1',
      oid: 'a1b2c3d4'.repeat(5),
      shortOid: 'a1b2c3d',
      message: 'fix: the thing\n\nand why',
    }
    eq(pr.requestAmend(request, 1000), true, 'an amend is accepted when there is a panel host')
    eq(seen, ['git'], 'and reveals the Git panel — the item is useless without the panel showing')
    ok(pr.amendPending(), 'the payload is parked for a panel that has not mounted yet')
    ok(woke > 0, 'and subscribers are told, so a panel that IS already mounted hears about it')

    // Claimed, not observed. The Git panel unmounts every time the user clicks another icon in
    // the activity rail; a request that survived its claim would mean that merely *looking* at
    // the panel later ticked Amend and replaced whatever was in the box.
    eq(pr.claimAmend(1200), request, 'the first claim gets the whole request back, verbatim')
    ok(!pr.amendPending(), 'and it is spent')
    eq(pr.claimAmend(1200), null, 'a second claim gets nothing — this is the rail-click bug')

    // The message is carried whole, body included. `CommitDetail.message` is the full message on
    // purpose (`show.rs` says why), and an amend that dropped the body would rewrite history by
    // deletion — silently, because the box is the only place it would have shown.
    ok(
      request.message.includes('\n\nand why'),
      'the request carries the full message rather than a summary line',
    )

    // The TTL. Without it a request parked for a panel the user never opened fires minutes later
    // when they open that panel for something else — and by then the oid names a commit that may
    // no longer be the tip.
    pr.requestAmend(request, 1000)
    eq(
      pr.claimAmend(1000 + pr.AMEND_TTL_MS + 1),
      null,
      'a request older than AMEND_TTL_MS is not honoured',
    )
    ok(!pr.amendPending(), '…and is spent by being looked at, not left for the next mount')
    pr.requestAmend(request, 1000)
    eq(pr.claimAmend(1000 + pr.AMEND_TTL_MS), request, 'the deadline itself is still inside')

    stop()
    const before = woke
    pr.requestAmend(request, 2000)
    eq(woke, before, 'the unsubscriber actually unsubscribes')
    pr.claimAmend(2000)
  }

  {
    // --- the draft, which is the one thing here with no undo ----------------------------------
    //
    // `planAmend` decides what happens when the commit box already holds a message. Appending
    // would produce a commit with two subject lines — an invisible corruption traded for a
    // visible loss. Refusing would make the user clear the box by hand and come back to the log
    // to repeat a gesture the app has already understood. So it asks, through the house
    // `ConfirmDestructive`, and Cancel drops the whole request rather than arming a rewrite of
    // HEAD with a message written for something else.
    const request = {
      project: 'proj-1',
      repo: 'repo-1',
      oid: 'b'.repeat(40),
      shortOid: 'b1b2b3b',
      message: 'feat: the committed subject',
    }

    eq(pr.planAmend('', request), { kind: 'adopt' }, 'an empty box is filled in silently')
    eq(
      pr.planAmend('   \n  ', request),
      { kind: 'adopt' },
      'and so is one holding only the whitespace a stray keystroke left — stopping to ask about ' +
        'that is how a user learns to click through this dialog without reading it',
    )

    const plan = pr.planAmend('wip: half a thought about a/b paths', request)
    eq(plan.kind, 'confirm', 'a real draft asks first')
    ok(
      plan.body.includes('wip: half a thought about a/b paths'),
      'and names the draft that would be lost — rule 1 of ConfirmDestructive, in the body ' +
        'because the file list runs every entry through basename/dirname and would draw this ' +
        'draft as the file `b paths` in the directory `wip: half a thought about a`',
    )
    ok(
      plan.body.includes(request.message),
      '…and names what would replace it, which the user cannot see yet',
    )
    ok(
      plan.body.includes(request.shortOid) && plan.confirmLabel.includes(request.shortOid),
      '…and names the commit, in the log’s own abbreviation rather than a second spelling',
    )
    ok(
      /Cancel/.test(plan.body),
      '…and says what Cancel does, because Cancel here drops the request rather than merely ' +
        'closing the dialog',
    )

    // The quote is a phrase inside a sentence, so a message with paragraphs in it must not
    // arrive as two sentences jammed together — a `\n` renders as nothing at all in a <p>.
    eq(
      pr.quoteDraft('subject\n\nbody line one\nbody line two'),
      'subject body line one body line two',
      'newlines and runs of spaces collapse to one space',
    )
    const long = 'x'.repeat(pr.QUOTE_LIMIT + 40)
    eq(pr.quoteDraft(long).length, pr.QUOTE_LIMIT, 'a long message is capped')
    ok(pr.quoteDraft(long).endsWith('…'), '…and says so with an ellipsis')
    eq(
      pr.quoteDraft('x'.repeat(pr.QUOTE_LIMIT)),
      'x'.repeat(pr.QUOTE_LIMIT),
      'a message exactly at the limit is quoted whole — git’s own wall is 72 columns, so ' +
        'the ordinary subject line arrives untrimmed',
    )

    pr.__resetPanelRequests()
  }

  // --- and the callers, which is what stops all of the above from being decoration ----------
  //
  // Source assertions. They prove the chain is spelled out, not that a click travels it — the
  // same thing `check-git-tree.mjs` says about its own.
  {
    const dispatch = readFileSync('src/keys/dispatch.ts', 'utf8')
    ok(
      !/deps\.showSidebar/.test(dispatch) && !/showSidebar\??:/.test(dispatch),
      '`DispatchDeps` no longer carries a `showSidebar` closure: one way to reveal a panel, not ' +
        'two. Two would be two sets of preconditions to keep in step, and the log’s Amend ' +
        'could reach neither',
    )
    ok(
      /case 'git\.commit': \{[\s\S]{0,2000}?if \(!requestPanel\('git'\)\) return unmet\(/.test(dispatch),
      'and `git.commit` reveals through the seam, still reporting when the window has no sidebar',
    )
    const app = readFileSync('src/App.tsx', 'utf8')
    ok(
      /registerPanelHost\(\(view\) => setSidebar\(\(s\) => showPanel\(s, view\)\)\)/.test(app)
        && /return \(\) => registerPanelHost\(null\)/.test(app),
      'App fills the slot with the same `showPanel` every other reveal goes through, and empties ' +
        'it on unmount — a root that went away while registered leaves `requestPanel` calling ' +
        'into a dead React tree',
    )
    ok(
      /if \(paneWindow\) return\s*\n\s*registerPanelHost\(/.test(app),
      '…and a detached-pane window registers nothing, so the dispatcher reports rather than ' +
        'moving a `useState` nothing draws',
    )
    const hook = readFileSync('src/sidebar/GitPanel/useGitPanel.ts', 'utf8')
    ok(
      /amendOf: amend && amendOf\?\.repo === unit\.repo \? amendOf\.oid : null/.test(hook),
      '`CommitRequest.amendOf` carries the oid the log named, and only to the repository it ' +
        'named it in. Without it `require_amend_head` has nothing to check and a HEAD that moved ' +
        'between the menu and the button is a silent rewrite of the wrong commit',
    )
    ok(
      /requestFocus\('commitMessage'\)/.test(hook),
      'adopting an amend also asks for the commit box — `CommitBox` is mounted under the Commit ' +
        'tab only, so an amend adopted while the panel is on Shelf would fill a box nobody can ' +
        'see, which is the "listed and does nothing" state the item was disabled to avoid',
    )
    ok(
      /const detail = explain\(e\)/.test(hook),
      '…and the refusal is a sentence: every `cmd/git.rs` command rejects with a serialised ' +
        '`GitError`, so `String(e)` is `[object Object]` — including for the `notHead` this ' +
        'whole path exists to provoke',
    )
  }

  // --- the clamp -------------------------------------------------------------------------

  eq(clampSidebarWidth(300), 300, 'a legal width is left alone')
  eq(clampSidebarWidth(40), SIDEBAR_MIN, 'a drag past the left limit stops at the floor')
  eq(clampSidebarWidth(9000), SIDEBAR_MAX, 'with no viewport, the static ceiling applies')
  eq(clampSidebarWidth(300.4), 300, 'widths are whole pixels')
  eq(
    clampSidebarWidth(Number.NaN),
    SIDEBAR_MIN,
    'a non-finite width becomes the floor rather than an invalid CSS declaration nobody reports',
  )

  // The viewport ceiling, which is the half that keeps the workspace alive. 720 is
  // `MIN_WIDTH` in `crates/cide-app/src/windows.rs` — the narrowest window the app allows.
  eq(
    sidebarCeiling(720),
    720 - RAIL_WIDTH - SPLITTER_WIDTH - MIN_WORKSPACE,
    'at the narrowest legal window the ceiling is what is left after the rail, the handle and ' +
      'the workspace — the handle is a flex item in the same row and its 6px come out of the ' +
      'workspace, so leaving it out would under-deliver MIN_WORKSPACE by exactly that much',
  )
  ok(
    sidebarCeiling(720) > SIDEBAR_MIN,
    'the narrowest window still leaves a usable range to drag in — floor below ceiling',
  )
  eq(sidebarCeiling(2560), SIDEBAR_MAX, 'on a wide monitor the static ceiling is the binding one')
  eq(
    sidebarCeiling(400),
    SIDEBAR_MIN,
    'on a window too narrow for both bounds the floor wins: a 90px panel is broken, not a compromise',
  )
  eq(sidebarCeiling(undefined), SIDEBAR_MAX, 'no viewport to ask falls back to the static ceiling')
  eq(clampSidebarWidth(600, 720), sidebarCeiling(720), 'the viewport ceiling beats the static one')

  // --- the gesture's arithmetic ----------------------------------------------------------

  eq(widthFromDrag(252, 60, 1440), 312, 'a drag right widens by exactly the pointer delta')
  eq(widthFromDrag(252, -60, 1440), 192, 'and a drag left narrows by it')
  eq(widthFromDrag(252, -400, 1440), SIDEBAR_MIN, 'a drag past the floor stops at it')
  eq(
    widthFromDrag(600, 400, 720),
    sidebarCeiling(720),
    'a drag on a narrow window stops where the workspace floor is, not at 640',
  )

  // The reason there is a width *per panel* at all: one panel's drag must not move another's.
  eq(
    withPanel({ files: 252, git: 420, agents: 320, ext: 252 }, 'files', 300),
    { files: 300, git: 420, agents: 320, ext: 252 },
    'resizing the explorer leaves the other widths where the user left them',
  )
  eq(
    withPanel({ files: 252, git: 420, agents: 320, ext: 252 }, 'git', 500),
    { files: 252, git: 500, agents: 320, ext: 252 },
    'and resizing the git panel leaves the explorer alone',
  )
  eq(
    withPanel({ files: 252, git: 420, agents: 320, ext: 252 }, 'agents', 500),
    { files: 252, git: 420, agents: 500, ext: 252 },
    'and so does resizing Agents — the M18 pair has its own number, not the explorer\'s',
  )
  // The clamp is shared, deliberately: `SidebarSettings::clamped()` in Rust applies one band
  // to every field, so a third width with a band of its own would be a width Rust silently
  // moved on the next snapshot.
  eq(
    withPanel({ files: 252, git: 420, agents: 320, ext: 252 }, 'agents', 9000),
    { files: 252, git: 420, agents: SIDEBAR_MAX, ext: 252 },
    'the agents width clamps to the same ceiling as the other two',
  )
  eq(
    withPanel({ files: 252, git: 420, agents: 320, ext: 252 }, 'agents', 10),
    { files: 252, git: 420, agents: SIDEBAR_MIN, ext: 252 },
    'and to the same floor',
  )

  // --- the persistence round trip --------------------------------------------------------

  const widths = { files: 300, git: 500, agents: 360, ext: 280 }
  eq(decodeWidths(encodeWidths(widths)), widths, 'a width survives encode → decode unchanged')
  eq(
    decodeWidths(
      encodeWidths({
        files: SIDEBAR_MIN,
        git: SIDEBAR_MAX,
        agents: SIDEBAR_MAX,
        ext: SIDEBAR_MIN,
      }),
    ),
    { files: SIDEBAR_MIN, git: SIDEBAR_MAX, agents: SIDEBAR_MAX, ext: SIDEBAR_MIN },
    'and so do both ends of the band — a clamp on the way back must not move a legal value',
  )
  eq(
    JSON.parse(encodeWidths(widths)),
    { filesWidth: 300, gitWidth: 500, agentsWidth: 360, extWidth: 280 },
    'the cache is written under the same wire names Rust stores, so the two can be compared by eye',
  )

  eq(decodeWidths(null), SIDEBAR_DEFAULT, 'a first launch gets the mock widths')
  eq(decodeWidths(''), SIDEBAR_DEFAULT, 'so does an empty entry')
  eq(decodeWidths('{'), SIDEBAR_DEFAULT, 'a half-written line must not stop the app from booting')
  eq(decodeWidths('[1,2]'), SIDEBAR_DEFAULT, 'nor must a value of the wrong shape')
  eq(decodeWidths('"252"'), SIDEBAR_DEFAULT, 'nor a bare string')
  eq(
    decodeWidths('{"filesWidth":"300","gitWidth":500,"agentsWidth":360,"extWidth":280}'),
    { files: SIDEBAR_DEFAULT.files, git: 500, agents: 360, ext: 280 },
    'a field of the wrong type falls back on its own without taking the others with it',
  )
  eq(
    decodeWidths('{"gitWidth":500,"agentsWidth":360,"extWidth":280}'),
    { files: SIDEBAR_DEFAULT.files, git: 500, agents: 360, ext: 280 },
    'a missing field takes its default — the shape a cache written by an older build has',
  )
  eq(
    decodeWidths('{"filesWidth":9000,"gitWidth":1,"agentsWidth":9000,"extWidth":9000}'),
    { files: SIDEBAR_MAX, git: SIDEBAR_MIN, agents: SIDEBAR_MAX, ext: SIDEBAR_MAX },
    'a cache edited by hand is clamped rather than trusted, and by the same band for every panel',
  )

  /*
   * The upgrade every existing user takes, and the reason it needs an assertion of its own.
   *
   * The cache line is versionless — deliberately, because it is a hint — so the *only* copy of
   * it any pre-M18 install has is this two-width object, and it is read on the very first
   * frame of the first launch after the upgrade, before Rust has answered `app.get_bootstrap`.
   * A missing `agentsWidth` that came back `undefined` or `NaN` would be written straight into
   * `--w-sidebar-agents`, and an invalid custom property is not an error anywhere: the browser
   * drops the declaration, the panel is drawn at whatever the cascade had or at nothing, and
   * no console says why. So the field must degrade to the default instead.
   */
  const preM18 = '{"filesWidth":300,"gitWidth":500}'
  eq(
    decodeWidths(preM18),
    { files: 300, git: 500, agents: SIDEBAR_DEFAULT.agents, ext: SIDEBAR_DEFAULT.ext },
    'a cache written before the agents width existed keeps its two widths and defaults the rest',
  )
  ok(
    Number.isFinite(decodeWidths(preM18).agents),
    'and the third is a real number — NaN here is a `NaNpx` declaration the browser silently drops',
  )
  ok(
    decodeWidths(preM18).agents === SIDEBAR_DEFAULT.agents,
    `…specifically ${SIDEBAR_DEFAULT.agents}, the token's value, so the first frame after an ` +
      `upgrade is the width a fresh install would show`,
  )

  eq(
    widthsFromSettings(null),
    SIDEBAR_DEFAULT,
    'a window that has not heard from Rust yet shows the defaults, not zero',
  )
  eq(
    widthsFromSettings({ filesWidth: 9000, gitWidth: 1, agentsWidth: 9000, extWidth: 1 }),
    { files: SIDEBAR_MAX, git: SIDEBAR_MIN, agents: SIDEBAR_MAX, ext: SIDEBAR_MIN },
    'and a hand-edited workspace.json is clamped on the way in, every field by the same band',
  )
  eq(
    toStored(widths),
    { filesWidth: 300, gitWidth: 500, agentsWidth: 360, extWidth: 280 },
    'the patch shape matches SettingsPatch',
  )
  eq(
    widthDeclarations(widths),
    [
      ['--w-sidebar-files', '300px'],
      ['--w-sidebar-git', '500px'],
      ['--w-sidebar-agents', '360px'],
      ['--w-sidebar-ext', '280px'],
    ],
    'the declarations carry units — a unitless custom property is silently invalid in a width',
  )
  // Every panel has to reach the DOM, or a width is stored, clamped and never drawn.
  eq(
    widthDeclarations(widths).map(([property]) => property),
    Object.values(SIDEBAR_TOKEN),
    'and there is one declaration per token — a panel missing from `widthDeclarations` is a ' +
      'width the drag stores and the paint never applies',
  )

  ok(SIDEBAR_CACHE_KEY.startsWith('cide.'), 'the cache key is namespaced to this app')

  // --- the same numbers, in the three other files that hold them -------------------------
  //
  // Every one of these is a copy that cannot be removed: CSS cannot import from TypeScript,
  // and Rust cannot import from either. So they are pinned instead.

  eq(
    Object.keys(SIDEBAR_TOKEN),
    ['files', 'git', 'agents', 'ext'],
    'the panels that own a width: the explorer (with search and problems), git, M18\'s ' +
      'Agents (with Tasks), and M22\'s one number for every contributed panel — four widths ' +
      'for a set of views that is no longer even finite, because a shared column keeps a ' +
      'shared number',
  )
  eq(
    Object.keys(SIDEBAR_DEFAULT),
    Object.keys(SIDEBAR_TOKEN),
    'and every one of them has a default, or a panel opens at no width at all',
  )

  const tokens = readFileSync('src/styles/tokens.css', 'utf8')
  for (const [panel, token] of Object.entries(SIDEBAR_TOKEN)) {
    const declared = new RegExp(`${token}:\\s*(\\d+)px`).exec(tokens)
    eq(
      declared ? Number(declared[1]) : null,
      SIDEBAR_DEFAULT[panel],
      `${token} in tokens.css is the default SIDEBAR_DEFAULT.${panel} falls back to`,
    )
  }
  eq(
    Number(/--h-rail:\s*(\d+)px/.exec(tokens)?.[1] ?? null),
    RAIL_WIDTH,
    'RAIL_WIDTH is the activity rail --h-rail actually reserves, or the ceiling is wrong by its width',
  )
  eq(
    Number(/--w-splitter:\s*(\d+)px/.exec(tokens)?.[1] ?? null),
    SPLITTER_WIDTH,
    'SPLITTER_WIDTH is what --w-splitter actually reserves — SidebarSplitter.module.css sizes ' +
      'the handle off the token, and the ceiling subtracts this number for it',
  )

  // The panels have to be reading the tokens, or writing them resizes nothing at all.
  const readers = cssModules('src').filter((path) => {
    const css = readFileSync(path, 'utf8')
    return Object.values(SIDEBAR_TOKEN).some((token) => css.includes(`width: var(${token})`))
  })
  ok(
    readers.length >= 4,
    `every sidebar panel sizes itself from a token — found ${readers.length}, expected the ` +
      `explorer, git, search and problems panels`,
  )

  const rust = readFileSync('../crates/cide-ipc/src/settings.rs', 'utf8')
  eq(
    Number(/SIDEBAR_MIN_WIDTH:\s*u16\s*=\s*(\d+)/.exec(rust)?.[1] ?? null),
    SIDEBAR_MIN,
    'the floor Rust clamps a patch to is the floor the drag stops at',
  )
  eq(
    Number(/SIDEBAR_MAX_WIDTH:\s*u16\s*=\s*(\d+)/.exec(rust)?.[1] ?? null),
    SIDEBAR_MAX,
    'and so is the ceiling — a width Rust rejects would spring back on the next snapshot',
  )
  const rustDefaults = /files_width:\s*(\d+),\s*git_width:\s*(\d+)/.exec(rust)
  eq(
    rustDefaults ? { files: Number(rustDefaults[1]), git: Number(rustDefaults[2]) } : null,
    { files: SIDEBAR_DEFAULT.files, git: SIDEBAR_DEFAULT.git },
    'SidebarSettings::default() is the same pair of widths as the tokens and SIDEBAR_DEFAULT',
  )
  // The agents default is a `fn` rather than a literal in the struct, because serde needs a
  // path — so it is read from there, and both of its uses are pinned below.
  eq(
    Number(/fn default_agents_width\(\)\s*->\s*u16\s*\{\s*(\d+)/.exec(rust)?.[1] ?? null),
    SIDEBAR_DEFAULT.agents,
    'and default_agents_width() is the third — the token, SIDEBAR_DEFAULT.agents and Rust agree',
  )
  ok(
    /agents_width:\s*default_agents_width\(\)/.test(rust),
    'SidebarSettings::default() uses that fn rather than a second copy of the number',
  )
  ok(
    /#\[serde\(default\s*=\s*"default_agents_width"\)\]/.test(rust),
    'and agents_width carries #[serde(default = …)], without which every pre-M18 ' +
      'workspace.json fails to deserialise its whole `sidebar` block on the first launch ' +
      'after the upgrade — the Rust half of the same degradation `decodeWidths` does for the cache',
  )
  // M22's width, the same three ways. A contributed panel is the first sidebar view whose set is
  // not known at build time, so this is the one number where a drift would show up as "the panel
  // some extension I installed opens at the wrong width", which nobody would report as a bug in
  // cide.
  eq(
    Number(/fn default_ext_width\(\)\s*->\s*u16\s*\{\s*(\d+)/.exec(rust)?.[1] ?? null),
    SIDEBAR_DEFAULT.ext,
    'default_ext_width() is the token, SIDEBAR_DEFAULT.ext and Rust agreeing on one number',
  )
  ok(
    /ext_width:\s*default_ext_width\(\)/.test(rust),
    'SidebarSettings::default() uses that fn rather than a second copy of the number',
  )
  ok(
    /#\[serde\(default\s*=\s*"default_ext_width"\)\]/.test(rust),
    'and ext_width carries #[serde(default = …)], for the reason agents_width does — a stored ' +
      'field without one takes the whole `sidebar` block down on the first launch after upgrade',
  )

  const windowsRs = readFileSync('../crates/cide-app/src/windows.rs', 'utf8')
  const minWidth = Number(/MIN_WIDTH:\s*f64\s*=\s*([\d.]+)/.exec(windowsRs)?.[1] ?? Number.NaN)
  ok(
    Number.isFinite(minWidth)
      && minWidth - RAIL_WIDTH - SPLITTER_WIDTH - MIN_WORKSPACE >= SIDEBAR_MIN,
    `the narrowest window the app allows (${minWidth}px) can still hold the rail, a ` +
      `${SIDEBAR_MIN}px sidebar, the handle and a ${MIN_WORKSPACE}px workspace — otherwise ` +
      `the clamp has no legal answer there`,
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `sidebar: ok (${Object.keys(SIDEBAR_TOKEN).length} panels, band ${SIDEBAR_MIN}-${SIDEBAR_MAX}px, ` +
      `${readers.length} stylesheets reading the tokens)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

/** Every `*.module.css` under `dir`, recursively. */
function cssModules(dir) {
  const found = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) found.push(...cssModules(path))
    else if (entry.name.endsWith('.module.css')) found.push(path)
  }
  return found
}
