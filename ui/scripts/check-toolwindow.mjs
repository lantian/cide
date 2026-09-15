/**
 * Checks the two import-free modules the git tool window's behaviour lives in —
 * `toolWindowHeight.ts` (how tall it is, and what survives a restart) and `toolWindowModel.ts`
 * (which tabs it has and which one is in front) — and pins the numbers against the three other
 * files that hold them.
 *
 * This exists because both halves fail *silently*, in the way `check-sidebar.mjs` was written
 * about. A clamp that lets a height through gives a panel that swallows the pane grid, with no
 * error anywhere. A boot cache that drops a field gives a panel that quietly returns to 260px on
 * every launch, which looks exactly like a feature that was never built. And a splitter whose
 * drag arithmetic has the wrong *sign* resizes, clamps and commits perfectly while moving the
 * opposite way to the pointer — no type catches that one, so it is asserted by hand below.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a `pointermove` moves the edge. There is no DOM in this process. That
 *     `setProperty('--h-toolwindow', …)` on `<html>` resizes the panel rests on
 *     `ToolWindow.module.css` saying `height: var(--h-toolwindow)`, which this file *does*
 *     check, and on the cascade, which it cannot.
 *   - that the drag avoids a React re-render. That is a property of the code path — nothing
 *     between `pointerdown` and `pointerup` calls into the store — and is argued in
 *     `ToolWindowSplitter.tsx`'s header, not measured here. It matters more here than for the
 *     sidebar: a vertical resize changes every pane's *rows*, so a re-render per move would
 *     `fit()` and SIGWINCH every live `claude` dozens of times in one gesture.
 *   - the round trip through Rust. `crates/cide-ipc/src/workspace.rs` owns that.
 *
 * Run: `pnpm --dir ui run check:toolwindow`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-toolwindow-'))
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
      'src/toolwindow/toolWindowHeight.ts',
      // Compiled beside it and for the same reason: which tab comes back after a close is a rule
      // with cases in it, and a rule that lives in a React state updater is a rule no check
      // script can run. Both files are import-free precisely so this one `tsc` can take them.
      'src/toolwindow/toolWindowModel.ts',
      // And the third: the Log tab's commit-list divider. Same reason again — the clamp is a
      // pixel fact expressed in per mille, which is exactly the kind of conversion that is right
      // in one direction and wrong in the other with nothing on screen to say so.
      'src/toolwindow/logSplit.ts',
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
    TOOL_TOKEN,
    TOOL_DEFAULT,
    TOOL_MIN,
    TOOL_MAX,
    TOOL_CACHE_KEY,
    CHROME_HEIGHT,
    HEADER_HEIGHT,
    TABSTRIP_HEIGHT,
    STATUS_HEIGHT,
    SPLITTER_HEIGHT,
    MIN_PANES,
    toolWindowCeiling,
    clampToolWindowHeight,
    heightFromDrag,
    heightDeclaration,
    geometryFromState,
    decodeToolWindow,
    encodeToolWindow,
  } = await import(`file://${join(out, 'toolWindowHeight.js')}`)

  const {
    TABS_INITIAL,
    tabRow,
    findHistory,
    openHistory,
    activate,
    closeHistory,
    showLog,
    toggle,
    restoreTabs,
    historyTitle,
    LOG_TAB_ID,
    walkTab,
  } = await import(`file://${join(out, 'toolWindowModel.js')}`)

  const {
    LOG_SPLIT_DEFAULT,
    LOG_SPLIT_HISTORY,
    LOG_LIST_MIN_PX,
    LOG_DETAILS_MIN_PX,
    LOG_STACK_BELOW_PX,
    clampLogSplit,
    splitFromDrag,
    splitFromKey,
    logLayout,
  } = await import(`file://${join(out, 'logSplit.js')}`)

  // --- the clamp -------------------------------------------------------------------------

  eq(clampToolWindowHeight(300), 300, 'a legal height passes through')
  eq(clampToolWindowHeight(10), TOOL_MIN, 'below the floor stops at the floor')
  eq(clampToolWindowHeight(99999), TOOL_MAX, 'with no viewport the static ceiling applies')
  eq(clampToolWindowHeight(260.4), 260, 'heights are whole pixels, so a stored integer round-trips')
  eq(
    clampToolWindowHeight(Number.NaN),
    TOOL_MIN,
    'NaN becomes the floor — NaN in a custom property is an invalid declaration nothing reports',
  )
  eq(heightDeclaration(260), [TOOL_TOKEN, '260px'], 'the declaration is the token and a px value')

  // --- the ceiling, which is what keeps the pane grid alive --------------------------------

  eq(
    toolWindowCeiling(1000),
    1000 - CHROME_HEIGHT - SPLITTER_HEIGHT - MIN_PANES,
    'the ceiling leaves the chrome, the handle and the pane floor',
  )
  eq(toolWindowCeiling(undefined), TOOL_MAX, 'no viewport means the static ceiling')
  eq(toolWindowCeiling(Number.NaN), TOOL_MAX, 'and so does a non-finite one')
  ok(toolWindowCeiling(300) === TOOL_MIN, 'a window too short for the band still yields the floor')
  eq(
    clampToolWindowHeight(800, 600),
    toolWindowCeiling(600),
    'a height past what fits is cut to what fits, not to TOOL_MAX',
  )
  eq(
    CHROME_HEIGHT,
    HEADER_HEIGHT + TABSTRIP_HEIGHT + STATUS_HEIGHT,
    'CHROME_HEIGHT is the three bars it claims to be',
  )

  // --- the drag, and the one thing no type catches -----------------------------------------
  //
  // The handle is on the tool window's TOP edge, so dragging down must SHRINK it. A splitter
  // with this backwards resizes, clamps and commits perfectly and moves the wrong way.

  ok(heightFromDrag(300, -50) > 300, 'dragging up (negative dy) GROWS the tool window')
  ok(heightFromDrag(300, 50) < 300, 'dragging down (positive dy) SHRINKS it')
  eq(heightFromDrag(300, -50), 350, 'and the magnitude is the pointer distance')
  eq(heightFromDrag(300, 0), 300, 'a drag that went nowhere changes nothing')
  eq(heightFromDrag(TOOL_MIN, 500), TOOL_MIN, 'dragging past the floor stops there')

  // --- the state a snapshot carries --------------------------------------------------------

  eq(
    geometryFromState(null),
    { open: false, height: TOOL_DEFAULT },
    'a pre-bootstrap window has no opinion and gets the closed default',
  )
  eq(
    geometryFromState({ open: true, height: 9000 }),
    { open: true, height: TOOL_MAX },
    'a hand-edited workspace.json is clamped on the way in',
  )

  // --- the boot cache ------------------------------------------------------------------------

  eq(TOOL_CACHE_KEY, 'cide.toolWindow', 'the cache key is stable, or every user loses their height once')
  const fallback = { open: false, height: TOOL_DEFAULT }
  eq(decodeToolWindow(null, 'w1'), fallback, 'no cache line is the closed default')
  eq(decodeToolWindow('', 'w1'), fallback, 'an empty one too')
  eq(decodeToolWindow('{"w1":', 'w1'), fallback, 'a half-written line must not stop the app booting')
  eq(decodeToolWindow('"nope"', 'w1'), fallback, 'nor must a line that is not an object')
  eq(decodeToolWindow('{"w2":{"open":true,"height":400}}', 'w1'), fallback, 'nor another window\'s entry')
  eq(
    decodeToolWindow('{"w1":{"open":true,"height":"tall"}}', 'w1'),
    { open: true, height: TOOL_DEFAULT },
    'a wrong-typed height falls back without losing the open flag',
  )
  eq(
    decodeToolWindow('{"w1":{"open":true,"height":9000}}', 'w1'),
    { open: true, height: TOOL_MAX },
    'and an out-of-band one is clamped rather than trusted',
  )

  const line = encodeToolWindow(null, 'w1', { open: true, height: 400 }, ['w1'])
  eq(decodeToolWindow(line, 'w1'), { open: true, height: 400 }, 'encode → decode round-trips')

  // The cross-window bleed, made unrepresentable in the cache the same way per-project scoping
  // makes it unrepresentable in Rust. `localStorage` is per ORIGIN, so every window shares this
  // one key; two windows writing a bare object instead of a map is the whole bug.
  const both = encodeToolWindow(line, 'w2', { open: false, height: 200 }, ['w1', 'w2'])
  eq(
    decodeToolWindow(both, 'w1'),
    { open: true, height: 400 },
    'writing window w2 leaves window w1 byte-identical',
  )
  eq(decodeToolWindow(both, 'w2'), { open: false, height: 200 }, 'and w2 is what w2 wrote')

  // Labels are uuids, so a PerProject user opening and closing projects would grow this for ever.
  const pruned = encodeToolWindow(both, 'w1', { open: true, height: 300 }, ['w1'])
  eq(decodeToolWindow(pruned, 'w2'), fallback, 'a window that is gone is pruned from the line')
  eq(decodeToolWindow(pruned, 'w1'), { open: true, height: 300 }, 'and the live one survives')

  // --- the tab row ---------------------------------------------------------------------------

  eq(TABS_INITIAL, { open: false, history: [], active: null }, 'a project with no saved state')
  eq(
    tabRow(TABS_INITIAL).map((r) => [r.id, r.label, r.active, r.closable]),
    [
      [null, 'Log', true, false],
      ['docker', 'Docker', false, false],
    ],
    'the Log tab is always there, always first, and never closable — and since M46 Docker is ' +
      'always second and never closable either: both are fixtures of the panel rather than ' +
      'things the user opened',
  )
  eq(historyTitle('crates/cide-git/src/log.rs'), 'log.rs', 'a tab is called its basename')
  eq(historyTitle('README.md'), 'README.md', 'including at the root')

  const one = openHistory(TABS_INITIAL, 'h1', 'r1', 'src/a.rs')
  ok(one.open, 'showing a history reveals the panel — every caller asked to SEE something')
  eq(one.active, 'h1', 'and puts it in front')
  eq(one.history.length, 1, 'and adds one tab')

  // Four menus reach this for the same file often: the tab strip, the file tree, the git changes
  // tree and the editor's own menu. Without the lookup, three right-clicks give three tabs.
  const again = openHistory(activate(one, null), 'h2', 'r1', 'src/a.rs')
  eq(again.history.length, 1, 'opening the same file twice does not append a second tab')
  eq(again.active, 'h1', 'it re-activates the one that is already there, keeping its id')

  // A path alone is ambiguous in a multi-root project where two repos both have `src/a.rs`.
  const twoRepos = openHistory(one, 'h3', 'r2', 'src/a.rs')
  eq(twoRepos.history.length, 2, 'the same path in a different repository is a different tab')
  eq(findHistory(twoRepos, 'r2', 'src/a.rs')?.id, 'h3', 'and is found by the pair')
  eq(findHistory(twoRepos, 'r9', 'src/a.rs'), null, 'an unopened pair is not found')

  const three = openHistory(openHistory(one, 'h2', 'r1', 'b.rs'), 'h3', 'r1', 'c.rs')
  eq(three.history.map((t) => t.id), ['h1', 'h2', 'h3'], 'tabs keep the order they were opened')
  eq(
    tabRow(three).map((r) => r.id),
    [null, 'docker', 'h1', 'h2', 'h3'],
    'and the Log tab still leads the row, with Docker behind it and every history tab after ' +
      'both — a fixture that moved as history tabs came and went would be a target that is ' +
      'never in the same place twice',
  )
  ok(
    tabRow(three).filter((r) => r.closable).length === 3,
    'every history tab is closable and the Log tab is not',
  )

  // The successor is the left neighbour, then the right, then Log — and never "nothing". A panel
  // that vanished because you closed one of its tabs reads as a crash rather than as a close.
  eq(closeHistory(three, 'h3').active, 'h2', 'closing the active tab falls to its LEFT neighbour')
  eq(
    closeHistory(activate(three, 'h1'), 'h1').active,
    'h2',
    'with no left neighbour it falls to the right',
  )
  eq(closeHistory(one, 'h1').active, null, 'closing the last one falls back to the Log tab')
  ok(closeHistory(one, 'h1').open, 'and the panel stays open')
  eq(
    closeHistory(activate(three, 'h1'), 'h3').active,
    'h1',
    'closing a tab that is not in front leaves the active one alone',
  )
  eq(closeHistory(three, 'nope'), three, 'closing a tab that is not there changes nothing')

  eq(activate(three, 'nope'), three, 'activating a tab that is not there changes nothing')
  eq(showLog(three).active, null, 'showLog puts the Log tab in front')
  ok(showLog({ ...three, open: false }).open, 'and reveals the panel')
  eq(toggle(TABS_INITIAL).open, true, 'the rail button opens a closed panel')
  eq(toggle(toggle(TABS_INITIAL)), TABS_INITIAL, 'and twice is a round trip that keeps the tabs')

  // Two repairs of states `validate` refuses to write but a hand-edited workspace.json can hold.
  eq(
    restoreTabs({ open: true, history: [], active: 'gone' }).active,
    null,
    'a dangling active would draw an empty body with nothing lit and no way back',
  )
  eq(
    restoreTabs({
      open: true,
      history: [
        { id: 'h1', repo: 'r', path: 'a', title: 'a' },
        { id: 'h1', repo: 'r', path: 'b', title: 'b' },
      ],
      active: 'h1',
    }).history.length,
    1,
    'a duplicate id would give two rows one React key and make the second unclickable',
  )

  // --- the id the Log tab walks under --------------------------------------------------------
  //
  // `LogRegistry` is keyed `(ProjectId, ToolTabId)`, and `ToolTabId` is `serde(transparent)` over
  // a `Uuid`. A sentinel that does not parse is rejected inside Tauri's argument deserialisation,
  // *before* the command body — so the failure is a rejected promise with no line of ours in it,
  // and the visible symptom is only that cancellation silently never works. Hence pinning the
  // shape here rather than trusting the literal.
  ok(
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(LOG_TAB_ID),
    'LOG_TAB_ID parses as a uuid, which is what ToolTabId deserialises into',
  )
  eq(walkTab(null), LOG_TAB_ID, 'the Log tab, which has no id of its own, walks under the sentinel')
  eq(walkTab('h1'), 'h1', 'a History tab walks under its own')

  // The nil uuid specifically, because it is the one value uuid v4 cannot mint: every other
  // choice is a value some future HistoryTabId could collide with, and a collision means one tab
  // cancelling another's walk — which reads as the log randomly returning half a page.
  eq(LOG_TAB_ID, '00000000-0000-0000-0000-000000000000', 'and it is the nil uuid')
  ok(
    tabRow(TABS_INITIAL).every((r) => r.id !== LOG_TAB_ID),
    'the sentinel never leaks into a tab row: the Log tab is still `id: null` everywhere it is stored',
  )

  // --- the commit list divider -------------------------------------------------------------
  //
  // The clamp is in pixels and the value is in per mille, because both minima are pixel facts:
  // a fraction means one thing at 1800px and something else at 700px, so clamping the fraction
  // directly lets the details pane vanish on a narrow window while the stored number still looks
  // reasonable.

  eq(clampLogSplit(500, 1000), 500, 'a legal split passes through')
  eq(
    clampLogSplit(50, 1000),
    Math.round((LOG_LIST_MIN_PX / 1000) * 1000),
    'a split that would starve the list is pushed out to the list floor',
  )
  eq(
    clampLogSplit(950, 1000),
    Math.round(((1000 - LOG_DETAILS_MIN_PX) / 1000) * 1000),
    'and one that would starve the details pane is pulled back to its floor',
  )
  eq(clampLogSplit(Number.NaN, 1000), LOG_SPLIT_DEFAULT, 'NaN becomes the default, never a NaN grid')
  eq(
    clampLogSplit(500, 300),
    500,
    'too narrow to honour both floors keeps the stored value — the caller stacks instead, and ' +
      'pinning it to an extreme would change the split because the user resized their window',
  )

  ok(splitFromDrag(500, 100, 1000) > 500, 'dragging right grows the commit list')
  ok(splitFromDrag(500, -100, 1000) < 500, 'and dragging left shrinks it')
  eq(splitFromDrag(500, 0, 1000), 500, 'a drag that went nowhere changes nothing')
  ok(splitFromKey(500, 1, 1000) > 500, 'an arrow key grows it too')
  eq(
    splitFromKey(splitFromKey(500, 1, 1000), -1, 1000),
    500,
    'and a step back is a round trip, so a held key does not drift',
  )

  eq(
    logLayout(LOG_STACK_BELOW_PX - 1, 450).mode,
    'stacked',
    'below the stacking width the pane stacks — two columns too tight to read is worse than one ' +
      'of each, and an inert separator is a keyboard stop that does nothing',
  )
  eq(logLayout(LOG_STACK_BELOW_PX, 450).mode, 'split', 'and at it the pane splits')
  eq(logLayout(1000, 500).list, 500, 'the list gets its share of the width, in pixels')
  ok(
    LOG_LIST_MIN_PX + LOG_DETAILS_MIN_PX <= LOG_STACK_BELOW_PX,
    'the stacking width is at least both floors, or there is a band with a legal split nobody draws',
  )

  // --- the same numbers, in the three other files that hold them ---------------------------
  //
  // Every one of these is a copy that cannot be removed: CSS cannot import from TypeScript, and
  // Rust cannot import from either. So they are pinned instead.

  const tokens = readFileSync('src/styles/tokens.css', 'utf8')
  eq(
    Number(new RegExp(`${TOOL_TOKEN}:\\s*(\\d+)px`).exec(tokens)?.[1] ?? null),
    TOOL_DEFAULT,
    `${TOOL_TOKEN} in tokens.css is the default TOOL_DEFAULT falls back to`,
  )
  // Read through the `calc()` the chrome font size wraps three of these four in. The literal
  // inside it is still the design height, which is what these constants are — the *rendered*
  // height is that times the scale, and `toolWindowCeiling` takes the scale as an argument so
  // both sides multiply by the same number. `--w-splitter` is a handle, not a text box, so it
  // does not scale and its declaration has no `calc()` to see through.
  for (const [token, value, what] of [
    ['--h-header', HEADER_HEIGHT, 'HEADER_HEIGHT'],
    ['--h-tabstrip', TABSTRIP_HEIGHT, 'TABSTRIP_HEIGHT'],
    ['--h-status', STATUS_HEIGHT, 'STATUS_HEIGHT'],
    ['--w-splitter', SPLITTER_HEIGHT, 'SPLITTER_HEIGHT'],
  ]) {
    eq(
      Number(new RegExp(`${token}:\\s*(?:calc\\()?(\\d+)px`).exec(tokens)?.[1] ?? null),
      value,
      `${what} is what ${token} actually reserves — the ceiling subtracts it`,
    )
  }

  const workspaceRs = readFileSync('../crates/cide-ipc/src/workspace.rs', 'utf8')
  eq(
    Number(/TOOL_WINDOW_MIN_HEIGHT:\s*u16\s*=\s*(\d+)/.exec(workspaceRs)?.[1] ?? null),
    TOOL_MIN,
    'the floor Rust clamps a patch to is the floor the drag stops at',
  )
  eq(
    Number(/TOOL_WINDOW_MAX_HEIGHT:\s*u16\s*=\s*(\d+)/.exec(workspaceRs)?.[1] ?? null),
    TOOL_MAX,
    'and so is the ceiling — a height Rust rejects would spring back on the next snapshot',
  )
  eq(
    Number(/log_split:\s*(\d+)/.exec(workspaceRs)?.[1] ?? null),
    LOG_SPLIT_DEFAULT,
    'ToolWindowState::default()\'s log_split is the same per-mille default as LOG_SPLIT_DEFAULT',
  )
  eq(
    Number(/height:\s*(\d+)\s*,/.exec(
      /impl Default for ToolWindowState \{[\s\S]*?\n\}/.exec(workspaceRs)?.[0] ?? '',
    )?.[1] ?? null),
    TOOL_DEFAULT,
    'ToolWindowState::default() is the same height as the token and TOOL_DEFAULT',
  )

  const windowsRs = readFileSync('../crates/cide-app/src/windows.rs', 'utf8')
  const minHeight = Number(/MIN_HEIGHT:\s*f64\s*=\s*([\d.]+)/.exec(windowsRs)?.[1] ?? Number.NaN)
  ok(
    Number.isFinite(minHeight)
      && minHeight - CHROME_HEIGHT - SPLITTER_HEIGHT - MIN_PANES >= TOOL_MIN,
    `the shortest window the app allows (${minHeight}px) can still hold the chrome, a ` +
      `${TOOL_MIN}px tool window, the handle and a ${MIN_PANES}px pane area — otherwise the ` +
      `clamp has no legal answer there`,
  )

  // --- the two split defaults, pinned against Rust (M21) --------------------------------------

  eq(LOG_SPLIT_DEFAULT, 450, "the Log tab's share")
  eq(
    LOG_SPLIT_HISTORY,
    500,
    'and a History tab is half: its right pane is one file\u2019s diff, which is the thing the tab '
      + 'was opened to read, where the Log tab\u2019s is a commit summary beside a wide list',
  )
  ok(
    workspaceRs.includes('pub split: Option<u16>'),
    'a History tab stores its OWN divider. One shared number would make the two fight \u2014 '
      + 'dragging the log wide would squeeze every history diff and back again',
  )
  ok(
    /None => next\.log_split = split/.test(readFileSync('../crates/cide-core/src/toolwindow.rs', 'utf8')),
    '\u2026and `set_layout` routes one wire field to whichever tab is in front, so a splitter '
      + 'that knows it dragged this panel need not know which kind of tab is showing',
  )

  // --- where the panel hangs in the tree -----------------------------------------------------
  //
  // This has been wrong twice, in opposite directions, and neither way is visible to anything
  // else in the suite: the chrome audit withholds the panel under `auditMode()`, the render
  // checks mount the pure view with no App around it, and CSS is not executed anywhere. What
  // decides the panel's width is one thing — which element it is a child of — so that is what is
  // pinned here.
  //
  //   inside `.content`       → spans only the panes, so opening Files narrows it
  //   sibling of `.body`      → spans the whole window, cutting off the activity rail
  //   last child of `.workArea` → everything but the rail, which is the one that was asked for

  const appTsx = readFileSync('src/App.tsx', 'utf8')
  const appCss = readFileSync('src/App.module.css', 'utf8')

  const railAt = appTsx.indexOf('<ActivityRail')
  const workAt = appTsx.indexOf('<div className={styles.workArea}>')
  const upperAt = appTsx.indexOf('<div className={styles.upper}>')
  const contentAt = appTsx.indexOf('<div className={styles.content}>')
  const panelAt = appTsx.indexOf('<ToolWindowHost')

  ok(workAt > railAt && railAt !== -1, 'the activity rail is rendered before `.workArea` opens')
  ok(
    !appTsx.slice(workAt, panelAt).includes('<ActivityRail'),
    'and outside it — the rail must not be inside `.workArea`, or the panel would cut it off '
      + 'and the icon bar would stop at the top of the git panel instead of running the full '
      + 'height of the window',
  )
  ok(
    upperAt > workAt && contentAt > upperAt && panelAt > contentAt,
    'the order is `.workArea` → `.upper` → `.content` → the panel, so the panel is a sibling of '
      + '`.upper` rather than anything inside it',
  )
  ok(
    appTsx.slice(contentAt, panelAt).includes('</div>'),
    '`.content` is closed before the panel opens: a panel still inside it would be as wide as '
      + 'the pane grid, and its left edge would move every time the sidebar was opened',
  )
  const afterPanel = appTsx.slice(panelAt)
  ok(
    afterPanel.indexOf('</div>') < afterPanel.indexOf('<StatusBar'),
    'and `.workArea` closes before the status bar, which is what keeps the panel above it',
  )

  // The two CSS lines the arrangement needs. Both are one property and both are invisible when
  // wrong in the same way — the layout simply overflows.
  ok(
    /\.workArea \{[^}]*flex-direction: column/.test(appCss),
    '`.workArea` is a column, or the panel would sit beside the working area instead of under it',
  )
  ok(
    /\.upper \{[^}]*min-height: 0/.test(appCss),
    '`.upper` has `min-height: 0`. Without it the row will not shrink below its content, so the '
      + 'panel takes its height from the container instead and pushes the pane grid and the '
      + 'status bar off the bottom of the window',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `tool window: ok (band ${TOOL_MIN}-${TOOL_MAX}px, default ${TOOL_DEFAULT}px, ` +
      `ceiling at the ${minHeight}px minimum window is ${toolWindowCeiling(minHeight)}px)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
