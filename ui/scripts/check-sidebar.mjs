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
  ok(!isPanelOpen({ view: 'settings', last: 'git' }), 'the settings view is not a panel')
  eq(
    toggleSidebar({ view: 'settings', last: 'git' }),
    { view: 'git', last: 'git' },
    'F4 from the settings view opens the last real panel',
  )
  eq(
    selectView({ view: 'files', last: 'files' }, 'settings'),
    { view: 'settings', last: 'files' },
    'and choosing ⚙ remembers the panel it replaced',
  )
  for (const from of [
    { view: 'files', last: 'files' },
    { view: null, last: 'search' },
    { view: 'settings', last: 'problems' },
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

  // The reason there are two stored widths at all: one panel's drag must not move the other.
  eq(
    withPanel({ files: 252, git: 420 }, 'files', 300),
    { files: 300, git: 420 },
    'resizing the explorer leaves the git width where the user left it',
  )
  eq(
    withPanel({ files: 252, git: 420 }, 'git', 500),
    { files: 252, git: 500 },
    'and resizing the git panel leaves the explorer alone',
  )

  // --- the persistence round trip --------------------------------------------------------

  const widths = { files: 300, git: 500 }
  eq(decodeWidths(encodeWidths(widths)), widths, 'a width survives encode → decode unchanged')
  eq(
    decodeWidths(encodeWidths({ files: SIDEBAR_MIN, git: SIDEBAR_MAX })),
    { files: SIDEBAR_MIN, git: SIDEBAR_MAX },
    'and so do both ends of the band — a clamp on the way back must not move a legal value',
  )
  eq(
    JSON.parse(encodeWidths(widths)),
    { filesWidth: 300, gitWidth: 500 },
    'the cache is written under the same wire names Rust stores, so the two can be compared by eye',
  )

  eq(decodeWidths(null), SIDEBAR_DEFAULT, 'a first launch gets the mock widths')
  eq(decodeWidths(''), SIDEBAR_DEFAULT, 'so does an empty entry')
  eq(decodeWidths('{'), SIDEBAR_DEFAULT, 'a half-written line must not stop the app from booting')
  eq(decodeWidths('[1,2]'), SIDEBAR_DEFAULT, 'nor must a value of the wrong shape')
  eq(decodeWidths('"252"'), SIDEBAR_DEFAULT, 'nor a bare string')
  eq(
    decodeWidths('{"filesWidth":"300","gitWidth":500}'),
    { files: SIDEBAR_DEFAULT.files, git: 500 },
    'a field of the wrong type falls back on its own without taking the other with it',
  )
  eq(
    decodeWidths('{"gitWidth":500}'),
    { files: SIDEBAR_DEFAULT.files, git: 500 },
    'a missing field takes its default — the shape a cache written by an older build has',
  )
  eq(
    decodeWidths('{"filesWidth":9000,"gitWidth":1}'),
    { files: SIDEBAR_MAX, git: SIDEBAR_MIN },
    'a cache edited by hand is clamped rather than trusted',
  )

  eq(
    widthsFromSettings(null),
    SIDEBAR_DEFAULT,
    'a window that has not heard from Rust yet shows the defaults, not zero',
  )
  eq(
    widthsFromSettings({ filesWidth: 9000, gitWidth: 1 }),
    { files: SIDEBAR_MAX, git: SIDEBAR_MIN },
    'and a hand-edited workspace.json is clamped on the way in',
  )
  eq(toStored(widths), { filesWidth: 300, gitWidth: 500 }, 'the patch shape matches SettingsPatch')
  eq(
    widthDeclarations(widths),
    [
      ['--w-sidebar-files', '300px'],
      ['--w-sidebar-git', '500px'],
    ],
    'the declarations carry units — a unitless custom property is silently invalid in a width',
  )

  ok(SIDEBAR_CACHE_KEY.startsWith('cide.'), 'the cache key is namespaced to this app')

  // --- the same numbers, in the three other files that hold them -------------------------
  //
  // Every one of these is a copy that cannot be removed: CSS cannot import from TypeScript,
  // and Rust cannot import from either. So they are pinned instead.

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
