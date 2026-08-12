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
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'

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
  ok(
    !/\bonAddRow=\{/.test(app),
    'App.tsx no longer feeds `onAddRow` to the pane tree — it would draw a second strip',
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
  //    longer clickable. `.frame` publishes `--pane-corner` and reserves exactly that width
  //    with `min-width`; every surface that puts something in that corner pads by it.
  ok(
    /--pane-corner:\s*\d+px/.test(/\.frame \{[^}]*\}/.exec(paneCss)?.[0] ?? ''),
    '`.frame` publishes `--pane-corner`, the width the cluster reserves',
  )
  for (const [file, selector] of [
    ['src/panes/DiffPane.module.css', '.header'],
    ['src/panes/GitDiffPane.module.css', '.header'],
    ['src/panes/EditorPane.module.css', '.conflict'],
  ]) {
    const rule = new RegExp(`\\${selector} \\{[^}]*\\}`).exec(css(file))?.[0] ?? ''
    ok(rule !== '', `${file} still has a \`${selector}\` rule to check`)
    ok(
      /var\(--pane-corner/.test(rule),
      `${file} \`${selector}\` reserves \`--pane-corner\` — its right-hand controls sit under `
        + 'the pane cluster otherwise, drawn but unclickable',
    )
  }

  // 4. And the detached window does not hide the cluster's buttons. Those are that window's
  //    only minimize/zoom/close — it carries no WM decorations — and the rule that used to
  //    hide the pane actions matched them too.
  ok(
    !/\[data-audit='paneTitle'\][^{]*\{[^}]*display:\s*none/.test(
      css('src/windows/DetachedPaneWindow.module.css'),
    ),
    'the detached window no longer `display: none`s the cluster — that is its close button',
  )

  if (failed === 0) console.log('rows layout: ok')
  else console.error(`\n${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed > 0 ? 1 : 0)
