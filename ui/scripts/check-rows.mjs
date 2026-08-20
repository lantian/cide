/**
 * The rows layout, proved on the markup.
 *
 * The user's complaint was that a vertical divider resized the whole window rather than the
 * row it sits in. That is now structural rather than arithmetic: a chain of same-axis splits
 * is rendered as **one** grid, and a divider is a track of *that* grid — so a vertical
 * divider's box is one row tall because its grid's box is. This script is what pins it.
 *
 * There is no browser and no jsdom in this harness — `ui/scripts/*.mjs` are SSR bundles run
 * under node — so nothing here measures a pixel. It asserts the track templates and the
 * containment structure instead, which is a *stronger* gate: invariance under a drag is
 * checked exactly, on the numbers the browser would be handed, rather than to within a pixel.
 *
 * Same harness as `check-diff-render.mjs`: SSR-bundle an entry, run it, read its digest.
 *
 * Run: `pnpm --dir ui run check:rows`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

/**
 * Every CSS module under `dir`, recursively.
 *
 * Enumerated rather than listed, because the assertion it serves — that nobody outside
 * `PaneTitleBar.module.css` reads the cluster's bare width — is only worth anything if a
 * *new* stylesheet reaching for the wrong property is caught. A hard-coded list is a list of
 * the files that were wrong last time. Same walker as `check-theme.mjs`'s.
 */
function cssModules(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...cssModules(path))
    else if (entry.name.endsWith('.module.css')) out.push(path)
  }
  return out
}

// Under `node_modules/.cache` rather than the system temp dir: the bundle keeps
// `react-dom/server` external, so node resolves it relative to wherever the output sits.
mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(join('node_modules/.cache', 'cide-rows-'))
let failed = 0
try {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/layout/rowsSmoke.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { stdio: 'inherit' },
  )

  // `@tauri-apps/api` touches `window` on import, and the tree imports the IPC types.
  globalThis.window = globalThis
  globalThis.location = { search: '' }
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  await import(`file://${resolve(out, 'rowsSmoke.js')}`)
  console.log = log

  const d = JSON.parse(printed.at(-1))

  const eq = (actual, expected, what) => {
    const a = JSON.stringify(actual)
    const b = JSON.stringify(expected)
    if (a !== b) {
      console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
      failed++
    }
  }
  const near = (actual, expected, what) => {
    const ok =
      Array.isArray(actual)
      && actual.length === expected.length
      && actual.every((v, i) => Math.abs(v - expected[i]) < 1e-9)
    if (!ok) {
      console.error(`FAIL ${what}\n  actual:   ${JSON.stringify(actual)}\n  expected: ~${JSON.stringify(expected)}`)
      failed++
    }
  }
  const ok = (cond, what) => {
    if (!cond) {
      console.error(`FAIL ${what}`)
      failed++
    }
  }

  // --- one grid per chain, and only per chain ---------------------------------------------

  const { chains, splitters } = d.plain
  eq(chains.length, 3, 'the Col spine and the two Row chains are three grids, not five')
  eq(
    chains.map((c) => c.axis),
    ['col', 'row', 'row'],
    'outermost is the spine; the rows nest inside it',
  )
  eq(
    chains.map((c) => c.depth),
    [0, 1, 1],
    'both rows are children of the spine, and neither is inside the other',
  )

  // --- the tracks are the shares, with a splitter track between each pair ------------------

  eq(
    chains[0].tracks,
    '0.5fr var(--w-splitter) 0.5fr',
    'the spine is two rows at 50/50 — the user asked for it in those words',
  )
  near(chains[1].fractions, [0.25, 0.25, 0.25, 0.25], 'row one is four equal tiles in one grid')
  near(chains[2].fractions, [0.5, 0.5], 'row two is two tiles, independent of row one')
  eq(
    chains.map((c) => c.tracks.split('var(--w-splitter)').length - 1),
    [1, 3, 1],
    'n members leave n-1 splitter tracks: a comb three levels deep still flattens to one grid',
  )

  // --- the claim the user actually made ---------------------------------------------------

  eq(splitters.length, 5, 'five splitters, one per SplitId in the fixture')
  const vertical = splitters.filter((s) => s.orientation === 'vertical')
  eq(vertical.length, 4, 'four vertical dividers: three in row one, one in row two')
  ok(
    vertical.every((s) => chains[s.chain]?.axis === 'row'),
    'every vertical divider is a track of a `row` chain',
  )
  ok(
    vertical.every((s) => s.chain !== 0),
    'and none of them is a track of the outermost grid — which is why one cannot span the window',
  )
  eq(
    splitters.filter((s) => s.orientation === 'horizontal').map((s) => s.chain),
    [0],
    'the one horizontal divider is the spine’s, and it is the only track that spans the tab',
  )

  // --- adding a tile must not remount the tiles already there -------------------------------
  //
  // React reconciles a chain's members by key. Every surviving member has to keep its key
  // across an insertion, or a live terminal is torn down and rebuilt for a gesture that
  // touched a different cell.

  eq(d.keys.before, ['pane-1', 'pane-2', 'pane-3', 'pane-4'], 'row one before the insertion')
  eq(
    d.keys.after,
    ['pane-1', 'pane-7', 'pane-2', 'pane-3', 'pane-4'],
    'and after it: every original key survives, in order, with the newcomer between them',
  )
  ok(d.keys.dividerZeroMoved, 'while `dividers[0]` does move — which is why it cannot be the key')
  ok(d.keys.chainRootHeld, 'the chain root keeps its id, so it can key the grid itself')

  // --- moving a divider copies every other track -------------------------------------------

  eq(d.drag.out, [0.375, 0.125, 0.25, 0.25], 'the pair splits 75/25 of its own 0.5')
  ok(d.drag.untouchedIdentical, 'members outside the pair are copied, not recomputed')
  eq(d.drag.pairSum, 0.5, 'and the pair keeps its own budget exactly, so nothing else has to give')
  eq(d.drag.inputUnchanged, '0.25,0.25,0.25,0.25', 'the input vector is not mutated')

  // --- the control the user could not find -------------------------------------------------

  eq(d.plain.addRow, 1, 'the `+ row` strip renders once')
  eq(
    d.plain.buttons,
    ['Add a row with a shell', 'Add a row with a Claude session'],
    'both rows a new row can hold are one click each, and both carry an accessible name',
  )
  eq(d.noStrip.addRow, 0, 'and nothing renders when the host offers no such gesture')
  eq(d.noStrip.splitters, 5, 'the tree still draws, and still resizes, without it')

  // --- divider k is the k-th split in IN-ORDER, which is the contract with Rust -----------
  //
  // The fixture above is a right-hand comb, where in-order and pre-order agree, so it cannot
  // see this. `Row(Row(a, b), c)` can: a drag sends `dividers[k]`'s id and a pair share, and
  // `cide-core::set_ratio` resolves that id back to `k` through its own in-order walk. Out of
  // step, the leftmost divider would resize the rightmost pair.

  eq(d.leaning.chains, 1, 'a left-leaning chain flattens to one grid too')
  near(d.leaning.fractions, [1 / 3, 1 / 3, 1 / 3], 'and to three equal thirds')
  eq(
    d.leaning.splitters,
    ['lean-inner', 'lean-outer'],
    'the leftmost divider is the inner split — pre-order would hand Rust the outer one',
  )

  // --- maximize still hides everything it used to -------------------------------------------

  eq(d.maximized.addRow, 0, 'the strip is withheld while a pane is maximized')
  eq(d.maximized.splittersByChain, [1, 3, 1], 'the fixture is unchanged by maximizing')
  eq(
    d.maximized.hiddenByChain,
    [1, 3, 0],
    'every divider on the path to the maximized pane is hidden — the spine’s and all three '
      + 'of row one’s. Row two’s is left alone on purpose: it is already inside a member the '
      + 'spine hid, and `visibility` inherits.',
  )

  // --- and that the one host which can supply the handlers actually does --------------------
  //
  // Everything above renders `SplitTree` from a fixture, so it proves the controls work when
  // they are wired — and says nothing about whether anything wires them. Both are withheld
  // when their callback is absent (the deliberate convention: a control that cannot act is
  // not drawn), and `App.tsx` is their only caller in the app. So the fixture suite passed
  // green with the feature completely unreachable, which is how this project has shipped a
  // dead control more than once — the search button, the project tabs, `installThemeSync`.
  //
  // A source assertion, and worth being exact about what that is worth: it proves the props
  // are passed, not that the values behind them reach Rust. The store actions they call are
  // covered on the Rust side. Making the props required on the interfaces would be a stronger
  // guarantee and was rejected — the fixtures above deliberately omit them to exercise the
  // withheld path, and that coverage is worth more than the compile-time check.
  // The add-TILE gesture is still a pane gesture, so `App.tsx` still supplies it.
  /*
   * A host coming back into the document asks for a frame.
   *
   * `parking` is never appended to `document.body`, so a parked host is detached, and a
   * detached element is non-intersecting — which pauses xterm's `RenderService`. The buffer
   * keeps taking writes the whole time, so the terminal's *state* stays right and only the
   * painting stops; the pane then comes back showing the frame it was parked on. That was
   * reported as "the pane still looks like it is working, and going full-screen fixes it",
   * full-screen being the only gesture on that path that resizes and therefore redraws.
   *
   * Parking is reached by a split, a project switch and a re-dock — never by a tab switch,
   * which flips `visibility` and leaves the slot mounted. So this is asserted on the source:
   * there is no DOM here, and the reproduction needs two projects and a turn that finishes
   * while one of them is hidden.
   */
  const paneHosts = readFileSync('src/layout/paneHosts.ts', 'utf8')
  const mount = paneHosts.slice(
    paneHosts.indexOf('export function mountHost('),
    paneHosts.indexOf('\nfunction quiesce('),
  )
  ok(
    mount !== '' && /term\.refresh\(0, term\.rows - 1\)/.test(mount),
    'mountHost repaints a host it just moved back into the document — a parked terminal is ' +
      'detached, xterm pauses its renderer while non-intersecting, and nothing else on the ' +
      'mount path ever asks for a frame',
  )
  ok(
    mount !== '' && /if \(moved\)/.test(mount),
    '…and only when it actually moved, so an ordinary re-render does not queue a full repaint ' +
      'of every pane on screen',
  )

  const app = readFileSync('src/App.tsx', 'utf8')
  ok(
    /\bonAddTile=\{/.test(app),
    'App.tsx passes `onAddTile` — without it the per-pane control renders nowhere',
  )

  // The add-ROW gesture moved into the header, because two sets of buttons for one gesture is
  // what the user reported. So the assertion moves with it rather than being deleted: the
  // header must MOUNT `RowControls`, and `App.tsx` must NOT re-supply `onAddRow` to the pane
  // tree, or both sets come back — which is exactly the regression this pair now pins.
  //
  // `RowControls` drives the workspace store itself when no handler is passed, so mounting it
  // is the whole of the wiring; there is no prop to check on the other side.
  const header = readFileSync('src/chrome/AppHeader.tsx', 'utf8')
  ok(/<RowControls\b/.test(header), 'the header mounts `RowControls`')
  // Scoped to the `<SplitTree>` element, not to the whole file. It used to grep all of
  // App.tsx, and that was too broad the moment the pane's title bar was deleted: the floating
  // controls' context menu takes an `onAddRow` of its own, on `<PaneFrame>`, which is a menu
  // item and not a second strip. The invariant was never "the string appears nowhere" — it is
  // "the pane TREE is not handed one", because that is what draws the strip.
  const splitTree = /<SplitTree\b[\s\S]*?renderPane=/.exec(app)?.[0] ?? ''
  ok(splitTree !== '', 'the App still mounts a `SplitTree` to check')
  ok(
    !/\bonAddRow=\{/.test(splitTree),
    '`<SplitTree>` is not handed `onAddRow` — it would draw a second strip under the panes',
  )

  // --- and that the header's own controls are still reachable at eight projects -------------
  //
  // `.tabs` is `overflow: hidden` and every tab in it is `flex: none`, so a control placed
  // *inside* it after the last tab is pushed out of the window once the tabs are wider than
  // the strip. That is what happened to `ProjectMenu` — the `+` that opens a project, and the
  // caret that reopens a recent one, both became unreachable at seven or eight projects, which
  // is the state a user cannot recover from without editing the persisted workspace by hand.
  //
  // Asserted on the source because there is no browser here to overfill a header in. What it
  // pins is the structural fact that caused the bug: `<ProjectMenu />` must be a sibling of the
  // strip, and the strip must not claim the slack (`flex: 1`) that the filler beside it now
  // takes. Both halves matter — moving the control out while leaving `.tabs` greedy would park
  // it against the window's right edge instead.
  // Matched from the `className` rather than from `<div` : the opening tag is multi-line now
  // (it carries a ref and a wheel handler), and the old one-line pattern silently matched
  // nothing — which reported as "the header has no tab strip" rather than as a stale regex.
  const tabsBlock = /className=\{styles\.tabs\}[\s\S]*?\n {6}<\/div>/.exec(header)?.[0] ?? ''
  ok(tabsBlock !== '', 'the header still has a `.tabs` strip to check')
  ok(
    !/<ProjectMenu\b/.test(tabsBlock) && /<ProjectMenu\b/.test(header),
    'the project `+`/recents control is mounted OUTSIDE the clipping tab strip',
  )
  const headerCss = readFileSync('src/chrome/AppHeader.module.css', 'utf8')
  const tabsRule = /\.tabs \{[^}]*\}/.exec(headerCss)?.[0] ?? ''
  // `flex-grow` read out of the shorthand rather than "the text is not `flex: 1;`": the whole
  // point is that this box must not take the header's slack, and `flex: 1 1 auto` takes it
  // just as `flex: 1` does while reading nothing like it. Shrink is deliberately not pinned —
  // it may shrink, it may not grow.
  const grow = /flex:\s*([\d.]+)/.exec(tabsRule)?.[1]
  ok(grow === '0', '`.tabs` does not claim the header’s slack — `.filler` does')
  // The strip **scrolls**, and this assertion replaced one that pinned `overflow: hidden`.
  //
  // That earlier fix moved the `+` and the caret out of the clipping box, which made *them*
  // reachable and left the tabs themselves unreachable: past the width of the strip a project
  // tab was simply gone, with no scrollbar and no wheel. Pinning `hidden` pinned half a fix.
  // Both halves are checked now — the control is outside the strip, and the strip can be
  // scrolled to the tab that is outside the view.
  ok(
    /overflow-x:\s*auto/.test(tabsRule) && /overflow-y:\s*hidden/.test(tabsRule),
    '`.tabs` scrolls horizontally and never vertically — a 34px header cannot hold a y scrollbar',
  )
  ok(
    /onWheel=/.test(header) && /scrollIntoView/.test(header),
    'and it is reachable: a wheel scrolls it, and the active tab scrolls itself into view',
  )

  // --- the pane bar asks WHICH pane, and asks it in the header's words ----------------------
  //
  // "'Add a pane to this row' - should have a dropdown to select - claude or bash". The two
  // kinds live in exactly one place so the header and the pane bar cannot drift, which is
  // already how `SplitTree`'s wording drifted from `RowControls`'.
  const rowControls = readFileSync('src/chrome/RowControls.tsx', 'utf8')
  ok(
    /export const PANE_KINDS\b/.test(rowControls)
      && /kind: 'shell'/.test(rowControls)
      && /kind: 'newClaude'/.test(rowControls),
    '`RowControls` exports the one `PANE_KINDS` table, holding both intents',
  )
  const titleBar = readFileSync('src/layout/PaneTitleBar.tsx', 'utf8')
  ok(
    /import \{ PANE_KINDS \}/.test(titleBar),
    'the pane title bar takes its two kinds from that table rather than restating them',
  )
  ok(
    /aria-haspopup="menu"/.test(titleBar) && /tileMenu\.openFor/.test(titleBar),
    'and its `⊞` opens a menu, so the kind is a choice rather than the domain’s default',
  )

  // --- the pane's chrome floats now, and must not sit on the pane's own controls ------------
  //
  // The 26px pane title bar is gone; ⊞ ⛶ ⧉ × float in each pane's top-right, over the content.
  // Three of the things this buys are silent when they break, so they are pinned here rather
  // than argued in a comment. There is still no browser in this harness, so these are read off
  // the stylesheets — which is where all three failures would live.
  //
  // Comments come out first, and not as tidiness: every rule below is quoted verbatim in some
  // comment nearby (that is the house style), so a check that greps the raw file passes on the
  // prose after the rule itself is deleted — which is exactly the "test that cannot fail" this
  // section exists to avoid. The first draft of assertion 4 did precisely that.
  const css = (path) => readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
  const paneCss = css('src/layout/PaneTitleBar.module.css')

  // 1. The cluster is out of flow. An in-flow cluster reserves a row per pane again, which is
  //    the 162px in a 2x3 grid the user asked us to give back.
  const clusterRule = /\.cluster \{[^}]*\}/.exec(paneCss)?.[0] ?? ''
  ok(
    /position:\s*absolute/.test(clusterRule),
    'the pane control cluster is out of flow — in flow it is the 26px bar again, per pane',
  )

  // 2. It hides with `opacity`, never `display`/`visibility`. That is the whole of the answer
  //    to "what does a user who cannot hover do": the controls stay in the tab order while
  //    invisible and `:focus-within` lights them up. `display: none` would look identical on a
  //    mouse and remove the only keyboard route to them.
  const revealRule = /\.reveal \{[^}]*\}/.exec(paneCss)?.[0] ?? ''
  ok(
    /opacity:\s*0/.test(revealRule)
      && !/display:\s*none/.test(revealRule)
      && !/visibility:\s*hidden/.test(revealRule),
    'the cluster hides with `opacity`, so Tab still reaches it — `display: none` would not',
  )

  // 3. Everything a pane draws in its own top-right stays clickable.
  //
  //    This is the regression that shipped and was found in review: the cluster takes the
  //    pointer over whatever is beneath it, and it reveals whenever the mouse is anywhere in
  //    the pane — so a control the pane drew there was still painted, still hovered, and no
  //    longer clickable. `.frame` publishes the band and every surface that puts something in
  //    that corner reserves it.
  //
  //    The property consumers read is `--pane-corner-clear`, NOT `--pane-corner`, and that
  //    distinction is the whole of the bug this loop grew a fourth entry for. `--pane-corner`
  //    is the cluster's *width*; the band it occupies is that width plus `--pane-cluster-inset`,
  //    which an editor pane sets to the minimap's 96px. `EditorPane`'s conflict bar reserved
  //    the width alone, was in this list, and passed — while both of its buttons sat wholly
  //    inside the cluster's live box, so the click that chose between the user's unsaved edits
  //    and what is on disk closed the pane. A gate that pins presence and not the right number
  //    protects the bug; this section now pins the number too.
  const frameRule = /\.frame \{[^}]*\}/.exec(paneCss)?.[0] ?? ''
  ok(
    /--pane-corner:\s*\d+px/.test(frameRule),
    '`.frame` publishes `--pane-corner`, the width the cluster reserves',
  )
  ok(
    /--pane-cluster-inset:\s*0px/.test(frameRule),
    '`.frame` states the default inset explicitly rather than leaving the sum below to ride a '
      + '`var()` fallback — a fallback resolves in silence, which is this reserve\'s failure mode',
  )
  ok(
    /--pane-corner-clear:\s*calc\(\s*var\(--pane-cluster-inset\)\s*\+\s*var\(--pane-corner\)\s*\)/
      .test(frameRule),
    '`--pane-corner-clear` is the SUM of the inset and the width — the band the cluster '
      + 'actually occupies, which is what a pane\'s own content has to keep clear',
  )
  //
  //    The find bar used to be a fourth entry here and is deliberately not one any more; the
  //    block below this loop pins the mechanism that replaced it, and pins that this rule has
  //    not quietly come back.
  for (const [file, selector] of [
    ['src/panes/DiffPane.module.css', '.header'],
    ['src/panes/GitDiffPane.module.css', '.header'],
    ['src/panes/EditorPane.module.css', '.conflict'],
  ]) {
    // The selector is matched literally rather than built into a character-escaped pattern:
    // the `"\\" + selector` trick these were originally written with escapes only the leading
    // character, and a selector carrying a `.` or a `(` would then match far too much.
    const body = css(file)
    const at = body.indexOf(`${selector} {`)
    const rule = at < 0 ? '' : body.slice(at, body.indexOf('}', at) + 1)
    ok(rule !== '', `${file} still has a \`${selector}\` rule to check`)
    ok(
      /var\(--pane-corner-clear/.test(rule),
      `${file} \`${selector}\` reserves \`--pane-corner-clear\` — its right-hand controls sit `
        + 'under the pane cluster otherwise, drawn but unclickable',
    )
  }

  // --- the find bar keeps its width, and the CLUSTER moves instead (M16) ------------------
  //
  // The reserve above is the right answer for a strip whose height can change with its content
  // — the conflict bar wraps on a narrow pane — and it was the wrong answer for the find bar,
  // which is a fixed 33px and is a *search field*: the user rejected a search box that is 221px
  // narrower than the file it is searching. So the bar spans the content and the cluster steps
  // below it, and this block is what makes that step provable rather than asserted twice.
  //
  // Four separate ways it could go wrong, each pinned here:
  //
  //   * the reserve creeps back onto the panel (a well-meaning revert) — the negative below;
  //   * the offset and the bar's height stop being the same number — both ends must name
  //     `--h-findbar`, and the token's value is re-derived from the bar's own parts;
  //   * the step fires for a strip it cannot size — the `:has()` rule must stay scoped to an
  //     editor pane and must keep excluding `[data-pane-strip='fluid']`;
  //   * the cluster slides off a short pane — the `top` must stay inside a `clamp()`.
  const surfaceCss = css('src/editor/EditorSurface.module.css')
  const panelAt = surfaceCss.indexOf('.body :global(.cm-panels.cm-panels-top) {')
  const panelRule =
    panelAt < 0 ? '' : surfaceCss.slice(panelAt, surfaceCss.indexOf('}', panelAt) + 1)
  ok(panelRule !== '', 'the find bar panel still has a rule of its own to check')
  ok(
    !/--pane-corner/.test(panelRule),
    'the find bar does NOT reserve the pane cluster any more. It is a search field, and one '
      + 'that stops 221px short of the file it is searching is what the reserve cost here; the '
      + 'cluster steps below it instead. Reintroducing `--pane-corner-clear` here would put the '
      + 'narrow bar back in silence, which is why this is a negative and not an omission',
  )
  ok(
    /margin-right:\s*var\(--w-minimap\)/.test(panelRule),
    'and it ends where the file content ends — `.cm-scroller` has the same `margin-right`, so '
      + 'the bar stops at the minimap instead of painting over its top 33px and taking its '
      + 'clicks (`.cm-panels` is z-index 300 and would win)',
  )
  ok(
    /height:\s*var\(--h-findbar\)/.test(panelRule),
    'and it is exactly `--h-findbar` tall — the same token the frame steps the cluster down by, '
      + 'so the offset and the thing being stepped over cannot desynchronise',
  )

  // Both setters of the step, with their selectors, out of the comment-stripped stylesheet.
  const stripSetters = [
    ...paneCss.matchAll(/([^{}]+)\{([^{}]*?--pane-top-strip:\s*([^;]+);[^{}]*)\}/g),
  ].map((m) => ({ selector: m[1].trim().replace(/\s+/g, ' '), value: m[3].trim() }))
  eq(
    stripSetters.length,
    2,
    'exactly two rules set `--pane-top-strip`: `.frame`, which states the 0 every pane kind '
      + 'has, and the single `:has()` rule that steps over a find bar. A third would be a '
      + 'second answer to "how far down does this pane\'s content start"',
  )
  const base = stripSetters.find((s) => s.selector === '.frame')
  const step = stripSetters.find((s) => s.selector !== '.frame')
  ok(
    base !== undefined && base.value === '0px',
    '`.frame` states the default strip explicitly rather than leaving the `top` below to ride a '
      + '`var()` fallback — the same rule `--pane-cluster-inset` follows, for the same reason: a '
      + 'fallback resolves in silence',
  )
  ok(
    step !== undefined && step.value === 'var(--h-findbar)',
    'and the step names `--h-findbar` rather than restating 33px, so it is the same number the '
      + 'panel above declares as its height',
  )
  if (step) {
    ok(
      /\[data-kind='editor'\]/.test(step.selector),
      'the step is scoped to an EDITOR pane. A diff draws its own 26px header above its '
        + 'CodeMirror surface, so a panel there is already below the cluster, and a terminal has '
        + `no panels at all — the rule reads: ${step.selector}`,
    )
    ok(
      /:has\(\s*:global\(\.cm-panels-top\)\s*\)/.test(step.selector),
      'and it tests for the panel CONTAINER, which `@codemirror/view` removes from the DOM at '
        + 'zero panels — an exact state test rather than a proxy for one',
    )
    ok(
      /:not\(\s*:has\(\[data-pane-strip='fluid'\]\)\s*\)/.test(step.selector),
      'and it stands down while a fluid-height strip is pinned above the panel. The conflict '
        + "bar's `.conflictText` is `flex: 1; min-width: 0` with no `white-space`, so it wraps "
        + 'and grows on a narrow pane: a fixed step over it would land the cluster in the middle '
        + 'of `Keep mine`. It also settles the both-strips case, where the find bar is no longer '
        + 'the topmost strip and nothing of it is under the cluster',
    )
  }

  // The declaring end of that exclusion. Without it the `:not()` above names an attribute
  // nobody sets, which is false for ever — the cluster would step down over a conflict bar and
  // through the button that preserves the user's unsaved edits.
  const editorPaneTsx = readFileSync('src/panes/EditorPane.tsx', 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/[^\n]*/g, '')
  ok(
    /className=\{styles\.conflict\}[\s\S]{0,400}?data-pane-strip="fluid"/.test(editorPaneTsx),
    'and the conflict bar actually declares `data-pane-strip="fluid"`. The `:not()` above is '
      + 'the only reader of it, so an attribute nobody sets makes that clause false for ever '
      + 'and steps the cluster down through `Keep mine`',
  )

  // The cluster's resting place, and the pane it cannot slide out of.
  ok(
    /top:\s*clamp\(\s*0px\s*,\s*var\(--pane-top-strip\)\s*,\s*calc\(100% - var\(--h-panetitle\)\)\s*\)/
      .test(clusterRule),
    'the cluster rests at `--pane-top-strip`, clamped into its own frame. `MIN_RATIO` is 0.1 '
      + 'and splits nest, so a pane under 60px is reachable by dragging, and `.frame` has no '
      + '`overflow: hidden` — an unclamped 33px there paints these controls over the pane BELOW. '
      + 'A bare `top: 0` would be the bug this whole block is about',
  )

  // --- `--h-findbar` is the bar's own parts, not a number somebody typed -------------------
  //
  // The panel's height is explicit, so this token being too small does not spill the bar — it
  // CLIPS it, silently, at whichever end the flex row gives way. Derived here from the four
  // declarations that actually make the height, so a taller input or a fatter padding fails on
  // this line instead of eating the close button.
  const tokensCss = css('src/styles/tokens.css')
  // Read through the `calc()` the chrome font size wraps every text box in. Three of the four
  // parts scale with it and the border does not, which is why the token below is `32 × scale
  // + 1px` rather than `33 × scale` — see `tokens.css`. The literals are still first inside
  // each `calc()`, which `check-ui-scale.mjs` is what enforces, so the design arithmetic is
  // still readable straight out of the stylesheet.
  const findBarPad = /\.findBar \{[\s\S]*?padding:\s*(?:calc\()?(\d+)px/.exec(surfaceCss)
  const findInputH = /\.findInput \{[\s\S]*?height:\s*(?:calc\()?(\d+)px/.exec(surfaceCss)
  const findButtonH = /\.findButton \{[\s\S]*?height:\s*(?:calc\()?(\d+)px/.exec(surfaceCss)
  const panelBorder = /border-bottom:\s*(\d+)px/.exec(panelRule)
  const findbarToken = /--h-findbar:\s*calc\((\d+)px \* var\(--ui-scale\) \+ (\d+)px\)/.exec(
    tokensCss,
  )
  ok(
    findBarPad !== null && findInputH !== null && findButtonH !== null && panelBorder !== null
      && findbarToken !== null,
    'the find bar still declares a padding, an input height, a button height and a bottom '
      + 'border, and `tokens.css` still declares `--h-findbar`',
  )
  if (findBarPad && findInputH && findButtonH && panelBorder && findbarToken) {
    const tallest = Math.max(Number(findInputH[1]), Number(findButtonH[1]))
    const scaled = Number(findBarPad[1]) * 2 + tallest
    const fixed = Number(panelBorder[1])
    eq(
      [Number(findbarToken[1]), Number(findbarToken[2])],
      [scaled, fixed],
      '`--h-findbar` is the bar it is measuring, split the way the chrome font size splits it: '
        + '5px of padding twice and the 22px control that sets the row height all scale '
        + '(`box-sizing: border-box`, so 22 is the whole control), and the panel\'s own 1px '
        + 'border-bottom does not',
    )
    eq(
      Number(findbarToken[1]) + Number(findbarToken[2]),
      33,
      'and that sum is 33px at the default chrome size. Spelled out as well as derived, '
        + 'because `--pane-corner` agreed with a wrong derivation for a whole milestone: two '
        + 'ways of being wrong have to disagree before either is worth trusting',
    )
    eq(
      Number(findbarToken[2]),
      fixed,
      'the *unscaled* half of the token is exactly the border and nothing else. Written as '
        + '`33px * var(--ui-scale)` the token was right at the default and short of its own '
        + 'parts below it — clipping the bar, which is the one failure this derivation exists '
        + 'to prevent, reintroduced by a multiplication',
    )
  }

  // --- and the step is only exact while there is exactly ONE top panel ---------------------
  //
  // `--h-findbar` is one bar's height. A second `Panel` with `top = true` would stack inside the
  // same `.cm-panels-top` container, which would then be taller than the step and the cluster
  // would come to rest on top of the second bar. `GoToLine.tsx` says why the stock `gotoLine`
  // dialog — a *bottom* panel — was not reused; this is the assertion that keeps the top group
  // a group of one.
  const editorFiles = readdirSync('src/editor', { withFileTypes: true })
    .filter((e) => e.isFile() && /\.tsx?$/.test(e.name))
    .map((e) => `src/editor/${e.name}`)
  const topPanelFiles = editorFiles.filter((f) =>
    /\btop\s*[=:]\s*true\b/.test(
      readFileSync(f, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, ''),
    ),
  )
  eq(
    topPanelFiles,
    ['src/editor/find.ts'],
    'the find bar is still the only TOP panel in the editor. A second one stacks inside the '
      + 'same `.cm-panels-top` box, which would then be taller than `--h-findbar` and leave the '
      + 'cluster resting on it — the exact collision the step exists to end',
  )

  // And nobody outside the publishing file reads the bare width any more. This is the
  // assertion that would have caught the conflict bar: it *was* wired to `--pane-corner`, and
  // `--pane-corner` is the wrong number on any pane kind that insets the cluster.
  const bareCornerReaders = cssModules('src')
    .filter((f) => f !== 'src/layout/PaneTitleBar.module.css')
    .filter((f) => /var\(\s*--pane-corner\s*[,)]/.test(css(f)))
  eq(
    bareCornerReaders,
    [],
    'no module outside `PaneTitleBar.module.css` reads bare `--pane-corner`. It is the '
      + "cluster's width, not the band it occupies; a consumer reading it reserves 125px where "
      + 'an editor pane needs 221 and leaves a 96px strip of its own controls unclickable',
  )

  // 4. And the detached window does not hide the cluster's buttons. Those are that window's
  //    only minimize/zoom/close — it carries no WM decorations — and the rule that used to
  //    hide the pane actions matched them too.
  ok(
    !/\[data-audit='paneTitle'\][^{]*\{[^}]*display:\s*none/.test(
      css('src/windows/DetachedPaneWindow.module.css'),
    ),
    'the detached window no longer `display: none`s the cluster — that is its close button',
  )

  // --- the floating cluster's reserve is a measurement, not a guess ---------------------
  //
  // `--pane-corner` is what four other stylesheets reserve (through `--pane-corner-clear`) so
  // their own right-aligned controls are not covered by the cluster, and it went stale the
  // moment the index and the title were removed from it: 126px of reserve for 115px of
  // controls. `min-width` holds the box open and `flex-end` pushes the buttons to its right
  // edge, so the slack showed up as the margin the user reported.
  //
  // Derived from the parts rather than restated, so the next control added to the cluster
  // fails here instead of quietly widening the gap again.
  //
  // The derivation itself was wrong until M13, in a way that matters more than an arithmetic
  // slip: it summed FOUR `.control`s, and `.control` is the *detached window's* button. The
  // docked cluster — the one every consumer is avoiding — is `.actions`, whose first child is
  // `.actionMenu` at `min-width: 26px`, not 22. It also forgot `.reveal`'s own `padding: 0 3px`
  // entirely. Two omissions, ten pixels, and the assertion was *labelled* "`--pane-corner`
  // equals the controls it reserves for" while agreeing with a number that did not. An
  // assertion that pins today's value rather than the property it names will do that.
  const px = (re) => Number(re.exec(paneCss)?.[1] ?? NaN)
  const reserve = px(/--pane-corner:\s*(\d+)px/)
  // The three plain actions, and the menu button, which is wider and sets its floor with
  // `min-width` because its content is an icon plus a caret.
  const action = px(/\.action \{[\s\S]*?width:\s*(\d+)px/)
  const actionMenu = px(/\.actionMenu \{[\s\S]*?min-width:\s*(\d+)px/)
  const marker = px(/\.marker \{[\s\S]*?width:\s*(\d+)px/)
  const markerMargin = /\.marker \{[\s\S]*?margin:\s*0 (\d+)px 0 (\d+)px/.exec(paneCss)
  const gap = px(/\.actions \{[\s\S]*?gap:\s*(\d+)px/)
  const pad = /\.cluster \{[\s\S]*?padding:\s*0 (\d+)px 0 (\d+)px/.exec(paneCss)
  const revealPad = px(/\.reveal \{[\s\S]*?padding:\s*0 (\d+)px/)
  ok(
    Number.isFinite(reserve) && Number.isFinite(action) && Number.isFinite(actionMenu)
      && Number.isFinite(revealPad) && markerMargin !== null && pad !== null,
    'the cluster still declares a reserve, an action size, a menu-button floor, the reveal '
      + 'padding, a marker and its own padding',
  )
  if (markerMargin && pad) {
    const want =
      Number(pad[1]) + Number(pad[2]) +          // `.cluster`  padding 0 2px      ->   4
      revealPad * 2 +                            // `.reveal`   padding 0 3px      ->   6
      actionMenu + action * 3 + gap * 3 +        // `.actions`  26 + 22*3 + 2*3    ->  98
      marker + Number(markerMargin[1]) + Number(markerMargin[2]) // `.marker`      ->  17
    eq(reserve, want, '`--pane-corner` equals the controls it reserves for')
    eq(reserve, 125,
      'and that sum is 125px. Spelled out as well as derived, because the derivation above '
        + 'agreed with 115 for a milestone by reading the wrong button: two ways of being '
        + 'wrong have to disagree before either is worth trusting',
    )
  }

  // And an editor insets it, because a 96px minimap lives where the cluster would land.
  ok(
    /\.frame\[data-kind='editor'\]\s*\{[^}]*--pane-cluster-inset:\s*var\(--w-minimap\)/.test(paneCss),
    "an editor pane insets the cluster by the minimap's width",
  )
  ok(
    /data-kind=\{pane\.kind\}/.test(readFileSync('src/layout/PaneTitleBar.tsx', 'utf8')),
    'the frame publishes the pane kind, without which that inset selects nothing',
  )

  // --- nothing inside a pane paints over its focus ring or its controls -----------------
  //
  // `.frame` isolates, so the ring (`z-index: 1`) and the cluster (`z-index: 2`) are scoped
  // to the pane — but `.body` was `position: relative; z-index: auto`, which is *not* a
  // stacking context, so a pane's contents competed with those two numbers directly.
  // CodeMirror's own layers are two orders of magnitude higher: opening the find bar in an
  // editor pane (Ctrl+F, a top-anchored `.cm-panels`) painted it over the top focus edge and
  // over the ⊞⛶⧉× cluster, and those four buttons could not be clicked while it was up.
  //
  // The comparison against the real numbers in `node_modules` is the point: it is what makes
  // this an assertion about a collision that exists rather than a restatement of the
  // stylesheet, and it is what will explain itself if a CodeMirror upgrade ever moves them.
  const ringZ = px(/\.frameFocused::after \{[^}]*z-index:\s*(-?\d+)/)
  const clusterZ = px(/\.cluster \{[^}]*z-index:\s*(-?\d+)/)
  const cmZ = [...readFileSync('node_modules/@codemirror/view/dist/index.js', 'utf8')
    .matchAll(/zIndex:\s*(\d+)/g)].map((m) => Number(m[1]))
  const cmMax = Math.max(0, ...cmZ)
  ok(
    cmMax > ringZ && cmMax > clusterZ,
    `CodeMirror still paints something (z-index ${cmMax}) above the pane's ring (${ringZ}) ` +
      `and cluster (${clusterZ}), so the containment below is load-bearing`,
  )
  ok(
    /\.body \{[^}]*position:\s*relative[^}]*\}/.test(paneCss) &&
      /\.body \{[^}]*z-index:\s*0[^}]*\}/.test(paneCss),
    'and the pane body is a stacking context — positioned AND with a numeric z-index — so ' +
      'those layers are scoped to the pane body instead of escaping over the frame',
  )

  // --- the ring is withheld from editor panes, and only the *paint* is withheld ----------
  //
  // The user asked for the focus ring back off file editors after living with it: two accent
  // pixels around the file you are reading are noise, and CodeMirror already says whether it
  // holds the caret. It stays on `claude`, `shell` and `diff` panes, where several terminals
  // share a tab and nothing else answers "which one am I typing into".
  //
  // This block exists because the rationale for *adding* the ring is still in
  // `PaneTitleBar.module.css` a dozen lines above the exception — it has to be, the ring is
  // still drawn for three kinds out of four — so the next reader of that paragraph is one
  // deletion away from putting it back on editors without noticing. Both halves are pinned:
  // the pseudo-element must generate no box at all, and the border must go back to `--border`
  // rather than merely being left at `--accent` with the second pixel gone.
  const focusedRule = /(^|\})\s*\.frameFocused \{([^}]*)\}/.exec(paneCss)?.[2] ?? ''
  ok(
    /border-color:\s*var\(--accent\)/.test(focusedRule),
    'a focused pane still lights its border with the accent — the ring exists for the kinds ' +
      'that kept it, and the editor exception below is meaningless without it',
  )
  // Both CodeMirror kinds, and `diff` is here on the user's decision rather than by symmetry.
  // The case for keeping it on a diff was that the pane is *answerable* — Accept / Reject, on a
  // turn a claude may be blocked on — but that is an argument about how much the pane matters,
  // and the ring answers "where do my keystrokes go". A diff answers that itself, the same way
  // an editor does, because it is the same surface.
  //
  // Driven as a loop rather than written twice: the two kinds must not drift into one having a
  // ring the other does not, which is the state the user has now corrected once.
  for (const kind of ['editor', 'diff']) {
    const border = new RegExp(
      `\\.frame\\[data-kind='${kind}'\\]\\.frameFocused[^{:]*\\{([^}]*)\\}`,
    ).exec(paneCss)?.[1]
    ok(
      border !== undefined && /border-color:\s*var\(--border\)/.test(border),
      `a focused ${kind} pane's border stays the ordinary \`--border\`, so the frame does not ` +
        'change colour under the caret',
    )
    ok(
      new RegExp(`\\.frame\\[data-kind='${kind}'\\]\\.frameFocused::after`).test(paneCss),
      `${kind} panes are named in the \`::after\` override too`,
    )
  }
  const ringOff = /\.frameFocused::after[^{]*\{([^}]*)\}/g
  ok(
    [...paneCss.matchAll(ringOff)].some(([, body]) => /content:\s*none/.test(body)),
    'and the override declares `content: none`, so the second accent pixel generates no box at ' +
      'all — `display: none` would leave one for a later edit to bring back',
  )
  // Specificity, not source order, is what makes those two win: the base rule is one class
  // (0,1,0) and each override is two classes plus an attribute (0,3,0). Asserted because a
  // future tidy that rewrote the override as a bare `.frameFocusedEditor` would still match
  // both greps above and would then lose the cascade to `.frameFocused`.
  for (const kind of ['editor', 'diff']) {
    for (const suffix of ['', '::after']) {
      const selector = `.frame[data-kind='${kind}'].frameFocused${suffix}`
      const classes = (selector.match(/\./g) ?? []).length
      ok(
        classes >= 2 && paneCss.includes(selector),
        `${selector} is written out, so it outranks the \`.frameFocused\` rule it overrides`,
      )
    }
  }
  //
  // And the half that must NOT have moved. Removing the ring removed a decoration; the focus
  // *state* behind it gates `file.save`, the find bar, the outline, Find Usages and
  // `pane.navigate.*` through `keys/context.ts`, and `editor/revealPane.ts` is explicit that
  // the two must not disagree. So the class is still applied from `focused` alone with no test
  // on the kind — the withholding happens in CSS, where it cannot reach the domain — and the
  // frame still publishes `data-focused`. Read from comment-stripped source, or the phrase
  // "data-focused" in the paragraph explaining it would satisfy this on its own.
  const paneTsx = readFileSync('src/layout/PaneTitleBar.tsx', 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/[^\n]*/g, '')
  ok(
    /className=\{focused \? `\$\{styles\.frame\} \$\{styles\.frameFocused\}` : styles\.frame\}/.test(
      paneTsx,
    ),
    'the frame still carries `frameFocused` for every pane kind — a `pane.kind === ' +
      "'editor'` test here would be the same pixels and a second source of truth about focus",
  )
  ok(
    /data-focused=\{focused \? 'true' : 'false'\}/.test(paneTsx),
    'and still publishes `data-focused`, which is the DOM view of `tree.focused` and the only ' +
      'thing left saying which pane the domain thinks is focused',
  )

  // --- every pane edge gets the same gutter, including the ones with no neighbour -------
  //
  // A splitter track is 6px, which is two 3px half-gutters back to back. At the edges of the
  // tree there is no neighbour and so there was no half-gutter: the outermost pane's frame sat
  // flush against the tab strip, the sidebar and the status bar, and two 1px `--border` lines
  // touching read as one 2px line belonging to neither box.
  //
  // This used to be justified by the editor's accent focus ring, which the block above has
  // just removed. The padding survives it: the gutter is about the frame's *ordinary* border,
  // which every pane of every kind carries in both focus states. The worst case is still the
  // same one — a **file tab** always opens as a single pane in a tab of its own, so every edge
  // is an outer edge and there is no interior gutter anywhere to set the scale by.
  //
  // Tied to `--w-splitter` rather than asserted as a literal, because the two numbers are one
  // decision: a splitter that stops being 6px leaves the edges out of step with the middle.
  const splitCss = css('src/layout/SplitTree.module.css')
  const canvasPad = Number(/\.canvas \{[^}]*padding:\s*(\d+)px/.exec(splitCss)?.[1] ?? NaN)
  const splitter = Number(
    /--w-splitter:\s*(\d+)px/.exec(readFileSync('src/styles/tokens.css', 'utf8'))?.[1] ?? NaN,
  )
  eq(
    canvasPad,
    splitter / 2,
    'the tree canvas pads by half a splitter, so a pane against the window chrome shows the ' +
      'same gutter as a pane against its neighbour rather than merging into it',
  )

  if (failed === 0) console.log('rows layout: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
