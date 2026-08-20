/**
 * Checks `src/settings/theme.ts` — the decisions a theme switch is made of — and walks the
 * terminal's slice of them against `src/styles/tokens.css`.
 *
 * This exists because the bug it guards is invisible from anywhere else. The app must never
 * be launched to look at a colour, there is no JS test runner here, and both halves of
 * "switching theme doesn't switch all claude/terminal windows" fail *silently*: a window
 * that never adopts the mirror's theme simply keeps painting, and an xterm handed an
 * unresolvable colour substitutes its own built-in default rather than complaining. So the
 * proof has to be the decision itself plus the stylesheet the decision reads.
 *
 * Same shape as `check-exit-marker.mjs`: `theme.ts` is import-free on purpose, so the
 * TypeScript in `node_modules` can compile it on its own.
 *
 * What this does NOT cover, and nothing here should be read as claiming:
 *   - that a repaint reaches the screen. There is no DOM and no WebGL context in this
 *     process; that `term.options.theme = …` propagates through xterm's `OptionsService` →
 *     `ThemeService` → `WebglRenderer._handleColorChange` → texture-atlas re-acquire was
 *     established by reading the xterm 6 / addon-webgl 0.19 sources, and is written down in
 *     the comment on `retheme` in `src/terminal/xterm.ts`.
 *   - that `cide://workspace-changed` actually reaches a second window. That seam is Rust's
 *     (`WorkspaceState::update` broadcasts to every window with the whole workspace, and
 *     `settings_set` bumps `rev` so the snapshot is not dropped as stale) and is covered by
 *     the Rust suite.
 *   - anything about the *look* of either palette beyond the terminal's tokens.
 *
 * Run: `pnpm --dir ui run check:theme`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-theme-'))
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
      'src/settings/theme.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      // The project sets it and this module is written for it: `palette[slot]` is
      // `string | undefined` here, and the fallbacks that handles are the difference
      // between "this token is missing" and a crash.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    DEFAULT_THEME,
    asThemeName,
    otherTheme,
    themeToAdopt,
    TERMINAL_SLOTS,
    TERMINAL_TOKENS,
    TERMINAL_GREY_RAMP,
    TERMINAL_MIN_CONTRAST,
    terminalPalette,
    unresolvedSlots,
    paletteSignature,
  } = await import(`file://${join(out, 'theme.js')}`)

  // --- what a window shows before anything tells it otherwise ---------------------------
  //
  // The user asked for this directly. Pinned rather than left to a default that reads the
  // same in three files, because the dark default was spread across Rust's `Theme::default`,
  // the store's seed and the bare `:root` block, and moving two of the three is a window
  // that boots dark and flips.
  eq(DEFAULT_THEME, 'light', 'the default theme is light')

  // --- the toggle -----------------------------------------------------------------------
  eq(otherTheme('light'), 'dark', 'the toggle leaves light for dark')
  eq(otherTheme('dark'), 'light', 'the toggle leaves dark for light')
  eq(otherTheme(otherTheme('light')), 'light', 'toggling twice is a round trip')

  // --- following another window -----------------------------------------------------------
  //
  // The reported bug, reduced to the one branch that was never running. Window B is showing
  // dark and a snapshot arrives saying light; without this answer being 'light', B keeps its
  // theme until it is restarted — which is what "doesn't switch all claude/terminal windows"
  // looks like from the outside.
  eq(
    themeToAdopt('dark', 'light', 'dark'),
    'light',
    'a window adopts the theme another window set',
  )
  eq(themeToAdopt('light', 'dark', 'light'), 'dark', 'and in the other direction')
  eq(
    themeToAdopt('light', 'light', 'dark'),
    null,
    'the window that made the change has nothing to adopt when its own write comes back — ' +
      'this runs inside a store subscription, and answering with a value here would write ' +
      'the store on every snapshot',
  )
  eq(
    themeToAdopt('light', undefined, undefined),
    null,
    'a window whose mirror has no settings yet has not been told anything and must not ' +
      'overwrite what it is showing',
  )
  eq(
    themeToAdopt('light', 'light', undefined),
    null,
    'bootstrap sets the mirror and the shown theme in one `set`, so there is nothing to adopt',
  )
  // The regression this argument exists for. Between the click and the snapshot the window
  // that made the change shows the new theme while the mirror still holds the old one; a
  // rule that only compared the two would answer 'dark' here and write the user's click
  // straight back out — leaving the switch to wait on an IPC round trip, or never happen if
  // it failed.
  eq(
    themeToAdopt('light', 'dark', 'dark'),
    null,
    'a local change waiting on its round trip is not reverted: the mirror did not move',
  )
  eq(
    themeToAdopt('light', 'dark', undefined),
    'dark',
    'but a mirror that has only just appeared and disagrees is still followed',
  )

  // Read back off `<html data-theme>` at install, ahead of the store's seed.
  eq(asThemeName('dark'), 'dark', 'the boot script’s attribute is taken at face value')
  eq(asThemeName('light'), 'light', 'in both directions')
  eq(asThemeName(undefined), null, 'an unset attribute falls through to the default')
  eq(asThemeName('beige'), null, 'and so does anything that is not a theme')

  // --- the terminal palette ---------------------------------------------------------------
  const slots = TERMINAL_SLOTS.map(([slot]) => slot)
  eq(new Set(slots).size, slots.length, 'no xterm slot is mapped twice')
  ok(slots.includes('background'), 'the terminal background is themed')
  ok(slots.includes('foreground'), 'the terminal foreground is themed')
  eq(slots.length, 21, 'every slot xterm is given: 5 UI colours plus the 16 ansi ones')

  const table = Object.fromEntries(TERMINAL_TOKENS.map((t) => [t, `value${t}`]))
  const resolved = terminalPalette((token) => table[token] ?? '')
  eq(Object.keys(resolved).length, slots.length, 'every slot is resolved, none dropped')
  eq(unresolvedSlots(resolved), [], 'a complete token table leaves nothing for xterm to invent')

  // The failure mode this whole table exists for: one token missing from one palette.
  // `parseColor` swallows it, so the only place it can be seen is here.
  const missingPanel = terminalPalette((token) => (token === '--panel' ? '' : 'x'))
  eq(
    unresolvedSlots(missingPanel),
    ['background', 'cursorAccent'],
    'a token the palette forgot is named, not silently replaced with xterm black',
  )

  // --- the signature `retheme` skips on ---------------------------------------------------
  //
  // If two different palettes could ever produce one signature, `retheme` would skip a
  // terminal that genuinely needed repainting — reintroducing the reported bug inside a
  // single window.
  const a = terminalPalette(() => 'aaa')
  const b = terminalPalette(() => 'bbb')
  ok(paletteSignature(a) !== paletteSignature(b), 'a changed palette changes the signature')
  eq(
    paletteSignature(terminalPalette((t) => table[t] ?? '')),
    paletteSignature(resolved),
    'the same palette read twice compares equal, so a redundant repaint is skipped',
  )
  const shuffled = Object.fromEntries([...Object.entries(resolved)].reverse())
  eq(
    paletteSignature(shuffled),
    paletteSignature(resolved),
    'the signature is taken in table order, so key insertion order cannot fake a change',
  )

  // --- and the stylesheet those tokens have to come from ----------------------------------
  const css = readFileSync('src/styles/tokens.css', 'utf8')
  const rules = leafRules(css)
  const paletteRules = rules.filter((r) => TERMINAL_TOKENS.some((t) => declares(r.body, t)))
  ok(paletteRules.length > 0, 'tokens.css defines a palette at all')

  // Only document-level rules are judged. A scoped override — `[data-theme='light'] .foo` —
  // is a legitimate thing to write and is not required to carry the whole palette.
  const themeRules = paletteRules.filter((r) => themeLevel(r.selector))

  for (const rule of themeRules) {
    const absent = TERMINAL_TOKENS.filter((t) => !declares(rule.body, t))
    eq(
      absent,
      [],
      `\`${rule.selector}\` carries part of the terminal palette, so it must carry all of ` +
        `it — a token left out here inherits the other theme's value, or none at all`,
    )
  }

  // Both themes have to be reachable: either the rule names them, or a base `:root` rule
  // supplies them and the theme rule overrides.
  const named = new Set(themeRules.flatMap((r) => themeNames(r.selector)))
  const hasBase = themeRules.some((r) => isBase(r.selector))
  for (const theme of ['dark', 'light']) {
    ok(named.has(theme) || hasBase, `the ${theme} palette is defined for terminals`)
  }

  // The point of the exercise. If these resolved alike, switching would be a no-op and
  // `retheme`'s signature check would correctly — and uselessly — skip every terminal.
  const dark = resolveTheme(themeRules, 'dark', [...TERMINAL_TOKENS, '--panel'])
  const light = resolveTheme(themeRules, 'light', [...TERMINAL_TOKENS, '--panel'])
  eq(
    TERMINAL_TOKENS.filter((t) => (dark[t] ?? '') === ''),
    [],
    'every terminal token resolves under dark',
  )
  eq(
    TERMINAL_TOKENS.filter((t) => (light[t] ?? '') === ''),
    [],
    'every terminal token resolves under light',
  )
  ok(
    paletteSignature(terminalPalette((t) => dark[t] ?? '')) !==
      paletteSignature(terminalPalette((t) => light[t] ?? '')),
    'the two palettes give terminals different colours, so a switch is visible in them',
  )

  // --- and whether those colours can actually be SEEN on that background ------------------
  //
  // Definedness is not the whole bug. ANSI black pointed at `--panel-2`, which is defined in
  // both palettes and is the right answer in dark (colour 0 sits just under the background,
  // as every dark scheme has it) — and in the new white theme resolved to #f6f6f9 on a
  // #ffffff terminal: 1.08:1. Anything a program printed in black was invisible, which from
  // the user's chair looks exactly like the theme switch this whole change is about failing
  // to reach that terminal. The gate above stayed green through all of it.
  //
  // The floor is `TERMINAL_MIN_CONTRAST` and is deliberately *not* restated here: the same
  // number is handed to xterm's `minimumContrastRatio` in `src/terminal/xterm.ts`, and the two
  // are one rule seen from two sides — this gate covers the backgrounds the palette paints,
  // that option covers the backgrounds a program paints for itself. A gate stricter than the
  // runtime floor ships colours it rejected; a looser one lets the runtime silently repaint
  // colours it approved. `theme.ts` carries the argument for the value.
  //
  // Judged against BOTH backgrounds this palette chooses. `--sel` was the gap: nothing checked
  // that selected text stays readable, and in a light theme the selection is the one large
  // tinted surface, so a colour can clear the terminal background and disappear the moment a
  // user drags across it.
  //
  // The exemptions are per theme AND per ground, and that asymmetry IS the rule rather than a
  // softening of it. A scheme is conventionally expected to sink the end of the greyscale ramp
  // that shares its background's lightness: in dark that is colours 0 and 8, which is what makes
  // 0 usable as a fill and 8 usable as "quiet", so `black` at 1.06:1 on #151518 is correct there
  // and would be a bug in a white terminal. In light the corresponding end is 15, and it is
  // exempted **on the selection only** — it must still clear the floor on `--panel`, because
  // that is the assertion that fails for a genuinely invisible palette. Set `--term-bright-white`
  // to #ffffff and the light case fails here while dark stays green.
  //
  // An exemption is a statement about a *fill*, not a licence to be invisible as ink. Nothing
  // exempted here reaches the screen at the ratio written beside it: xterm's
  // `minimumContrastRatio` repaints every one of them the moment the slot is used as a
  // foreground, which is the whole reason the two halves share a number. The second assertion in
  // this loop is what keeps that true — an exemption for a slot that would pass anyway is a dead
  // one, and a dead exemption is how the next regression walks past this gate.
  const INK_FLOOR = TERMINAL_MIN_CONTRAST
  const QUIET = {
    '--panel': { dark: ['black', 'brightBlack'], light: [] },
    '--sel': { dark: ['black', 'brightBlack'], light: ['brightWhite'] },
  }
  const inkSlots = TERMINAL_SLOTS.filter(
    ([slot]) => !['background', 'cursorAccent', 'selectionBackground'].includes(slot),
  )
  const belowFloor = (palette, ground, slots) =>
    slots
      .map(([slot, token]) => [slot, palette[token], contrast(palette[token], ground)])
      .filter(([, , ratio]) => ratio !== null && ratio < INK_FLOOR)
      .map(([slot, value, ratio]) => `${slot}=${value} ${ratio.toFixed(2)}:1`)

  for (const [name, palette] of [['dark', dark], ['light', light]]) {
    for (const [groundToken, quiet] of Object.entries(QUIET)) {
      const ground = palette[groundToken]
      eq(
        belowFloor(palette, ground, inkSlots.filter(([slot]) => !quiet[name].includes(slot))),
        [],
        `every ink colour clears ${INK_FLOOR}:1 on the ${name} theme's ${groundToken} ${ground}`,
      )
      eq(
        quiet[name].filter(
          (slot) => belowFloor(palette, ground, inkSlots.filter(([s]) => s === slot)).length === 0,
        ),
        [],
        `every slot exempted on the ${name} theme's ${groundToken} is one that would fail — an ` +
          `exemption the palette has outgrown stops being a decision and starts being a hole`,
      )
    }
  }

  // The floor itself, pinned rather than only read.
  //
  // Reading `TERMINAL_MIN_CONTRAST` above is what keeps this gate and xterm's
  // `minimumContrastRatio` equal to each other. It is not what keeps them equal to *something*.
  // At 1 they are equal to nothing: xterm's `_applyMinimumContrast` returns before it does
  // anything when the ratio is 1 (`DomRendererRowFactory.ts:481`, `TextureAtlas.ts:394`), and
  // `ratio < 1` is unsatisfiable, so every assertion in the loop above passes against any palette
  // whatsoever. That edit was made and run: one character, all 25 check scripts green, the whole
  // change gone. So the value is pinned here the way `DEFAULT_THEME` is — the app reads the
  // constant, this file states what the constant is allowed to be.
  ok(TERMINAL_MIN_CONTRAST > 1, 'the ink floor is above 1, which is xterm’s "do not correct"')
  eq(
    TERMINAL_MIN_CONTRAST,
    3,
    'the ink floor is 3:1 — where "a human cannot see this at all" lives, rather than AA’s 4.5, ' +
      'which would drag every deliberately quiet colour up to the weight of the loud ones',
  )

  // --- and whether the four greys are still in the order every program assumes -------------
  //
  // A ratio floor is a claim about visibility and says nothing about identity. Colours 0, 8, 7
  // and 15 are the one run of the palette whose *ordering* is a contract: a program picks
  // between them expecting 0 darkest and 15 lightest, and picks 7 or 15 as a **background**
  // expecting a light bar. The light theme used to fail that while passing everything above —
  // 7 was `--text` (#1b1b1f) and 15 `--text-hi` (#0a0a0c), so 15 was darker than 7 and both were
  // darker than 8, and `ESC[47m` painted a near-black status bar in a white terminal. No
  // contrast option can catch it either: backgrounds are never adjusted.
  //
  // Strictly increasing rather than non-decreasing, so a theme cannot quietly collapse two of
  // the four into one colour — which is the state the light palette was in, with 0, 7 and the
  // default foreground all within a few points of #1b1b1f.
  //
  // Membership is pinned before ordering is checked, because a ramp with a slot dropped out of it
  // still rises: shortening `TERMINAL_GREY_RAMP` would turn the assertion below green by giving
  // it less to look at, which is the one way a check of this shape goes quiet without failing.
  eq(
    [...TERMINAL_GREY_RAMP],
    ['black', 'brightBlack', 'white', 'brightWhite'],
    'the ramp is still all four ANSI greys, in the order ANSI gives them',
  )
  for (const [name, palette] of [['dark', dark], ['light', light]]) {
    const ramp = TERMINAL_GREY_RAMP.map((slot) => {
      const entry = TERMINAL_SLOTS.find(([s]) => s === slot)
      const value = entry ? palette[entry[1]] : undefined
      return { slot, value, l: luminance(value) }
    })
    // A step whose value is not a hex colour fails here rather than being skipped: `luminance`
    // answers null for a `var()` or an `rgba()`, and a ramp this file cannot order is a ramp
    // nothing is checking.
    const rises = (step, i) =>
      i === 0 || (step.l !== null && ramp[i - 1].l !== null && step.l > ramp[i - 1].l)
    const wrong = ramp.filter((step, i) => !rises(step, i)).map((s) => `${s.slot}=${s.value}`)
    eq(
      wrong,
      [],
      `the ${name} greyscale ramp rises across ${TERMINAL_GREY_RAMP.join(' < ')} — a program ` +
        `that picks between ANSI 0, 8, 7 and 15 is picking on that order, and one that sets 7 ` +
        `or 15 as a background is asking for the light end of it`,
    )
  }

  // --- every token any stylesheet asks for, not just the terminal's -----------------------
  //
  // The gap this closes, found by review after it had already shipped: a CSS module wrote
  // `var(--scrim, rgba(0,0,0,.42))` for a token `tokens.css` defined in neither palette. CSS
  // has no undefined-variable error — it silently takes the fallback — so both themes kept
  // painting the dark literal, the change was a no-op in the light theme it was written for,
  // and all thirteen check scripts stayed green. The terminal half above catches exactly this
  // for `TERMINAL_TOKENS`; there was nothing watching the other ~40 stylesheets.
  //
  // Resolved against every document-level rule in `tokens.css`, not only the palette ones,
  // because the structural tokens (`--font-ui`, the fixed chrome heights) live in a separate
  // `:root` block and are legitimately theme-independent.
  const docRules = rules.filter((r) => themeLevel(r.selector))
  const referenced = new Map()
  for (const file of cssModules('src')) {
    const body = readFileSync(file, 'utf8')
    // Properties a module defines for itself are not tokens.css's to supply.
    const local = new Set([...body.matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1]))
    for (const m of body.matchAll(/var\(\s*(--[\w-]+)/g)) {
      if (local.has(m[1])) continue
      if (!referenced.has(m[1])) referenced.set(m[1], file)
    }
  }
  ok(referenced.size > 0, 'the stylesheets reference tokens at all')

  /*
   * The one thing that is a `var()` and is *not* tokens.css's to supply: a custom property one
   * CSS module publishes for another.
   *
   * `--pane-corner-clear` is the band the floating control cluster occupies in every pane's
   * top-right: the cluster's own width plus however far in from the right edge the pane kind
   * parks it. Four pane surfaces draw something in that corner — two diff headers, the editor's
   * conflict bar and the find bar — and reserve it so the cluster does not sit on them, which
   * makes it a measurement passed between modules rather than a colour. It cannot go in
   * tokens.css: it is one box's geometry, it is a `calc()` over two properties published in the
   * same rule, and a theme has no opinion on it.
   *
   * It still has to resolve, and for the same reason as everything above: the consumers write
   * `var(--pane-corner-clear, 0px)`, so a renamed or deleted publisher turns the reserve into a
   * silent zero and puts a diff's layout switcher back underneath the pane's ⧉ — drawn, hovered
   * and unclickable, which is the state review found it in. So the exemption is not a blanket
   * one: the property is excused from the palette walk only while some module actually defines
   * it, and the publisher list is pinned below so a second one is a decision rather than a
   * surprise.
   *
   * `--pane-corner` and `--pane-cluster-inset`, the two halves of that sum, do not appear here:
   * they are defined and read inside `PaneTitleBar.module.css` alone, which the walk above
   * treats as local. That is the shape `check:rows` now enforces — a consumer reading the bare
   * width reserves 125px where an editor pane needs 221, which is how the conflict bar came to
   * be drawn, hovered, and answering the close button's click.
   */
  const publishers = new Map()
  for (const file of cssModules('src')) {
    for (const m of readFileSync(file, 'utf8').matchAll(/(--[\w-]+)\s*:/g)) {
      if (!publishers.has(m[1])) publishers.set(m[1], [])
      if (!publishers.get(m[1]).includes(file)) publishers.get(m[1]).push(file)
    }
  }

  const wanted = [...referenced.keys()].sort()
  const darkAll = resolveTheme(docRules, 'dark', wanted)
  const lightAll = resolveTheme(docRules, 'light', wanted)
  // Unresolved AND published by exactly one module is the cross-module contract; anything else
  // that fails to resolve is the silent-fallback bug this section was written for.
  const contract = (t) => publishers.get(t)?.length === 1
  for (const theme of [['dark', darkAll], ['light', lightAll]]) {
    eq(
      wanted
        .filter((t) => (theme[1][t] ?? '') === '' && !contract(t))
        .map((t) => `${t} (${referenced.get(t)})`),
      [],
      `every token the stylesheets use resolves under ${theme[0]} — a missing one takes its ` +
        `\`var()\` fallback in silence`,
    )
  }
  eq(
    wanted
      .filter((t) => (darkAll[t] ?? '') === '' && contract(t))
      .map((t) => `${t} <- ${publishers.get(t)[0]}`),
    ['--pane-corner-clear <- src/layout/PaneTitleBar.module.css'],
    'the only custom property one CSS module publishes to another is the pane corner reserve, ' +
      'and exactly one file publishes it',
  )

  // --- typography: never ask the engine for a face this app did not bundle ----------------
  //
  // Same class of bug as the palette above, and the same reason it lives in a script: it
  // fails *silently*. A stylesheet asking for a weight or a style with no bundled face does
  // not error — the engine picks the nearest real face and, unless `font-synthesis` says
  // otherwise, emboldens or shears it. That is the "fonts are ugly" report: `<h3>`, `<b>` and
  // `<strong>` were being fake-bolded out of the 600 face because the UA sheet asks for 700
  // and the UI face is imported at 400/500/600, and five surfaces asking `font-style: italic`
  // were getting an obliqued upright because no italic was imported at all.
  //
  // What this proves: the imports in `fonts.css`, the declarations in the modules, and the
  // one rule that normalises the UA's 700 agree with each other. What it cannot prove is what
  // reaches a screen — there is no engine in this process. `font-synthesis: none` is asserted
  // because it is the backstop that turns any future disagreement into a visibly wrong weight
  // rather than an invisible smear.
  const fontsCss = readFileSync('src/styles/fonts.css', 'utf8')
  const bundled = new Map()
  for (const m of fontsCss.matchAll(/@import '@fontsource\/([\w-]+)\/(\d+)(-italic)?\.css'/g)) {
    const faces = bundled.get(m[1]) ?? { weights: new Set(), italics: new Set() }
    ;(m[3] ? faces.italics : faces.weights).add(Number(m[2]))
    bundled.set(m[1], faces)
  }
  ok(bundled.size === 2, '`fonts.css` still imports exactly two families from @fontsource')

  // The map from a package name to the family a stylesheet names. Asserted rather than
  // assumed: everything below reads `--font-ui`/`--font-mono` as "the Inter stack" and "the
  // JetBrains Mono stack", and a swapped stack would make every check here vacuous.
  //
  // The UI family is Inter because that is what IDEA ships and what the reference capture is
  // drawn in; it replaced IBM Plex Sans outright rather than joining it, which is why the
  // count above is still two. `UI_FAMILY` is the single place that name is written down —
  // the bold check at the bottom reads it too, and hard-coding it twice is how that check
  // came to assert about a family that was no longer the UI face.
  const UI_FAMILY = { pkg: 'inter', css: "'Inter'" }
  const stacks = resolveTheme(docRules, 'light', ['--font-ui', '--font-mono'])
  ok(
    (stacks['--font-ui'] ?? '').startsWith(UI_FAMILY.css),
    `\`--font-ui\` still leads with the family \`fonts.css\` imports as \`${UI_FAMILY.pkg}\``,
  )
  ok(
    (stacks['--font-mono'] ?? '').startsWith("'JetBrains Mono'"),
    '`--font-mono` still leads with the family `fonts.css` imports as `jetbrains-mono`',
  )

  // Both families are asked for an italic — `ContextMenu` and `ProxySection` in the UI face,
  // the editor's comment token and every terminal that receives SGR 3 in the mono one — so
  // both must ship one. Without a face here the engine shears the upright, which is the
  // single most visible synthesis artefact in an editor because it is every comment.
  for (const [pkg, faces] of bundled) {
    ok(faces.italics.size > 0, `\`${pkg}\` bundles a real italic rather than leaving it synthesised`)
  }

  // A keyword is the trap, not a number: `bold` means 700, no stylesheet that writes it is
  // thinking about which faces ship, and the UI family has no 700. Numbers are checked against
  // the union of both families because a rule's family is not decidable from its text — this
  // catches `800`, not a mono-only 700 written into a UI stylesheet.
  const everyWeight = [...bundled.values()].flatMap((f) => [...f.weights, ...f.italics])
  const badWeights = []
  for (const file of [...cssModules('src'), 'src/styles/tokens.css']) {
    const body = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
    for (const m of body.matchAll(/font-weight:\s*([^;}]+)/g)) {
      const value = m[1].trim()
      if (/^var\(/.test(value) || value === 'inherit' || value === 'normal') continue
      const weight = Number(value)
      if (!Number.isFinite(weight) || !everyWeight.includes(weight)) {
        badWeights.push(`${file}: font-weight: ${value}`)
      }
    }
  }
  eq(badWeights, [], 'every `font-weight` in the stylesheets is a weight `fonts.css` imports')

  // --- the sidebar lists are the UI face, and the code inside them is not -------------------
  //
  // A deliberate departure from the mock, which specifies "21px mono rows" for the explorer.
  // Side by side with IDEA the monospace tree reads as a terminal listing rather than a list
  // of names, and it is the one piece of feedback this panel has drawn twice. The reason it
  // survived the first pass is instructive: that pass checked JetBrains Mono was *loading* —
  // it was, and that was never the question — and recorded the rows as correct.
  //
  // So it is pinned here, because "restore the mock's mono rows" is a plausible-looking edit
  // that a future reader of `Grount IDE.dc.html` would make in good faith, and nothing else
  // in the repo would contradict them.
  //
  // The second half matters as much as the first. A hit row in the search panel carries a
  // path *and* a line of source, and only the path is a name: `.lineText` must stay mono or
  // the match highlight sits over glyphs of a width the file does not have. Checking only
  // "the trees are proportional" would pass a change that swept the source preview along
  // with the names, which is the more damaging half of the mistake.
  for (const [file, uiSelectors, monoSelectors] of [
    // `.tag` is in the mono list because it was the third way to get this wrong. It stated no
    // family at all and *inherited* mono from its row, so the day the row went proportional
    // the status letter followed it silently — and a proportional `M` at the row's 13px is
    // 11.74px in a 9px right-aligned column, painting over the gap before the name. Inherited
    // is not stated; that is the whole point of this loop.
    ['src/sidebar/FileTree.module.css', ['.row', '.rename'], ['.tag']],
    ['src/sidebar/GitPanel/ChangesTree.module.css', ['.fileName', '.dirName'], ['.count']],
    ['src/sidebar/SearchPanel.module.css', ['.fileRow'], ['.lineText', '.lineNo']],
    /*
     * The overlays, added in M15 — and the reason it was added is a finding rather than a
     * tidy-up.
     *
     * This table is *precisely* the gate for "a name is the UI face, code is the mono face", and
     * `Overlay.module.css` was not in it. So when M14 grew the Find usages popup — a list of file
     * headings each followed by lines of source, the same shape as the three files above — the
     * one overlay that needed this sweep was the one file the sweep did not visit. The result
     * shipped: the heading and the source line differed by half a pixel of font size and nothing
     * else, and the user's report was "it doesn't see where is filename and where is code".
     *
     * `.usageAt` is in the mono list for the same reason `.tag` is above: it is a *column* (a
     * right-aligned line number), and a proportional digit column jitters by a glyph per row.
     */
    [
      'src/overlays/Overlay.module.css',
      ['.usageFileName'],
      ['.usageText', '.usageAt', '.container', '.where'],
    ],
  ]) {
    const rules = leafRules(readFileSync(file, 'utf8'))
    for (const [selectors, want, why] of [
      [uiSelectors, '--font-ui', 'a name, so the proportional face'],
      [monoSelectors, '--font-mono', 'code or a column, so the mono face'],
    ]) {
      for (const selector of selectors) {
        const rule = rules.find((r) => parts(r.selector).includes(selector))
        ok(rule !== undefined, `${file} still has a \`${selector}\` rule`)
        // Stated, not inherited. `.lineText` inherited mono from its row for as long as the
        // row was mono, and the day the row changed it silently followed — which is exactly
        // the regression this pins, so an inherited pass would be no check at all.
        //
        // Not `declares()`: that asks whether a rule *defines* the custom property, and none
        // of these do — they consume it. The first draft of this loop used it and failed all
        // eight, including the three that were already right.
        const family = rule && /font-family:\s*var\((--font-[a-z]+)\)/.exec(rule.body)?.[1]
        ok(
          family === want,
          `${file} \`${selector}\` sets its own \`font-family: var(${want})\` — ${why}` +
            (family && family !== want ? ` (found ${family})` : family ? '' : ' (found none)'),
        )
      }
    }
  }


  /*
   * The Find usages heading against the source line under it — the same file, the same list, and
   * the thing the family sweep above cannot see.
   *
   * A family split is necessary and is not sufficient: the shipped bug had the *right* families
   * available and used the wrong one for the heading, and even after fixing the family a heading
   * at the source line's size and weight would still read as another line of code. So this asks
   * for a difference on the two axes a reader actually uses at a glance — size and weight — and
   * for the one declaration that was written to make the heading stand out to not be a no-op.
   */
  {
    const rules = leafRules(readFileSync('src/overlays/Overlay.module.css', 'utf8'))
    const rule = (selector) => rules.find((r) => parts(r.selector).includes(selector))?.body ?? ''
    /*
     * The design size behind a declaration, whether it is a literal or a ladder token.
     *
     * Chrome font sizes are `var(--fs-ui-11-5)` since the chrome gained a font-size setting,
     * and the number in that name *is* the design size — `tokens.css` defines the token as
     * `calc(11.5px * var(--ui-scale))`. So the comparison below still compares the two numbers
     * the mock specifies; it just reads them through a name. Kept as a resolver rather than
     * relaxed to "any number in the value", because `calc(11.5px * var(--ui-scale))` contains
     * a `1` before it contains an `11.5`.
     */
    const number = (body, prop) => {
      const raw = new RegExp(`${prop}:\\s*([^;]+)`).exec(body)?.[1]?.trim()
      if (raw === undefined) return NaN
      const token = /^var\(--fs-ui-([\d-]+)\)$/.exec(raw)
      if (token) return Number(token[1].replace('-', '.'))
      return Number(/^([0-9.]+)/.exec(raw)?.[1])
    }

    const head = rule('.usageFileName')
    const code = rule('.usageText')
    ok(head !== '' && code !== '', 'the usage heading and the usage source line both have rules')
    ok(
      number(head, 'font-size') > number(code, 'font-size'),
      'a usage file heading is LARGER than the source lines under it. The shipped version was '
        + '11px against 11.5px — the heading was the smaller of the two — and with the same '
        + 'family, weight and colour that half-pixel was the entire distinction. The report was '
        + '"it doesn\'t see where is filename and where is code"',
    )
    ok(
      /font-weight:/.test(head) && !/font-weight:/.test(code),
      'and it carries a weight the source line does not',
    )
    ok(
      /font-weight:\s*var\(--w-bold\)/.test(head),
      'through the token, not the literal `600`: a literal is how a mono rule ends up asking for '
        + 'a weight its family does not ship, CSS matching walks up to 700, and the emphasis '
        + 'lands two steps heavy — `SearchPanel.module.css` records that exact afternoon, and '
        + 'the weight sweep in this file cannot catch it because it checks against the union of '
        + 'both families',
    )
    ok(
      /direction:\s*rtl/.test(head),
      'and it ellipsises from the FRONT — the tail of `crates/cide-lsp/src/progress.rs` is the '
        + 'filename, which is the one part of a heading that must survive truncation',
    )
    ok(
      /flex:\s*none/.test(rule('.usageCount')),
      'the hit count beside it does not flex. It used to be drawn with `.group`, which is '
        + '`flex: 1`, so a two-character number claimed half a 620px card from a path that was '
        + 'already truncating from the wrong end',
    )
    ok(
      /background:/.test(rule('.usageFile')) && !/background:\s*var\(--chrome\)/.test(rule('.usageFile')),
      'and the heading row\'s ground is not `--chrome`, which is exactly `.card`\'s own ground — '
        + 'the one declaration written to make the heading stand out was painting it the colour '
        + 'it already was',
    )
  }

  // --- the three sidebar lists are one row height, and the icon centres on a whole pixel ---
  //
  // Two invariants that had no gate, and both had already been broken once by hand.
  //
  // The first is that the row height is written down in three places and nothing connected
  // them: `FileTree.tsx` and `SearchPanel.tsx` each hand a number to their own virtualizer's
  // `estimateSize`, and `ChangesTree.module.css` states it in CSS because that tree is not
  // virtualized. They read 21, 21 and 23 — a disagreement nobody chose, against an IDEA that
  // uses one height for both its trees.
  //
  // The second is the reason the first matters beyond tidiness. All three lists centre a
  // `<FileIcon>` with `align-items: center`, so it sits at `(row − icon) / 2`, and a half
  // pixel there softens the 1px horizontal edges the Material set is drawn on — a whole
  // column of slightly blurred glyphs, which is what `icons/FileIcon.module.css` exists to
  // document. That parity is a *relationship* between two numbers in different files, so
  // either one can be edited alone and look locally correct: 15px was right for a 21px row
  // and wrong for a 24px one, 16px the reverse. Asserting the relationship is the only way
  // that stays true, and it is why this checks `% 2` rather than pinning either literal.
  const rowHeights = [
    ['src/sidebar/FileTree.tsx', /const ROW_HEIGHT = (\d+)/],
    ['src/sidebar/SearchPanel.tsx', /const ROW_HEIGHT = (\d+)/],
  ].map(([file, re]) => [file, Number(re.exec(readFileSync(file, 'utf8'))?.[1])])
  const gitRow = leafRules(readFileSync('src/sidebar/GitPanel/ChangesTree.module.css', 'utf8'))
    .find((r) => parts(r.selector).includes('.row'))
  /*
   * Read through the `calc()` the chrome font size wraps every text box in.
   *
   * `.row` is `calc(24px * var(--ui-scale))` now, and the 24 is still the design height this
   * check is about — `check-ui-scale.mjs` is what guarantees the literal stays first inside
   * the `calc()`, precisely so derivations like this one keep working. The two `ROW_HEIGHT`
   * constants above are the same design number in JavaScript; they are multiplied by the same
   * scale at render time in `settings/useUiScale.ts`, so agreeing here is agreeing everywhere.
   */
  rowHeights.push([
    'src/sidebar/GitPanel/ChangesTree.module.css',
    Number(/height:\s*(?:calc\()?(\d+)px/.exec(gitRow?.body ?? '')?.[1]),
  ])
  for (const [file, height] of rowHeights) {
    ok(Number.isFinite(height), `${file} still states a row height this script can read`)
  }
  eq(
    [...new Set(rowHeights.map(([, h]) => h))],
    [rowHeights[0][1]],
    'the explorer, the search panel and the git changes tree are all one row height — '
      + rowHeights.map(([f, h]) => `${h} in ${f.split('/').pop()}`).join(', '),
  )
  const iconBox = Number(
    /width:\s*(\d+)px/.exec(
      leafRules(readFileSync('src/icons/FileIcon.module.css', 'utf8'))
        .find((r) => parts(r.selector).includes('.icon'))?.body ?? '',
    )?.[1],
  )
  ok(Number.isFinite(iconBox), 'icons/FileIcon.module.css still states a width for `.icon`')
  ok(
    (rowHeights[0][1] - iconBox) % 2 === 0,
    `the ${iconBox}px icon box centres on a whole pixel in a ${rowHeights[0][1]}px row — `
      + `(${rowHeights[0][1]} − ${iconBox}) / 2 = ${(rowHeights[0][1] - iconBox) / 2}`,
  )

  // The specific elements that were muddy. The UA sheet bolds them to 700 and no stylesheet
  // says otherwise, so the normalisation has to be here or it is nowhere.
  const uaBold = leafRules(readFileSync('src/styles/tokens.css', 'utf8')).find((r) =>
    /(^|,\s*)strong$/.test(r.selector),
  )
  ok(uaBold !== undefined, 'tokens.css still normalises the elements the UA stylesheet bolds')
  const uaParts = parts(uaBold?.selector ?? '')
  eq(
    ['h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'b', 'strong'].filter((el) => !uaParts.includes(el)),
    [],
    'and it covers every one of them — a heading level left out is fake-bolded again',
  )
  // Read through `UI_FAMILY` rather than a second hard-coded package name. When this said
  // `'ibm-plex-sans'` literally, swapping the UI face to Inter left the line *passing* while
  // asserting about a family the app no longer paints — green, and meaningless. Asserting the
  // family is bundled at all is what keeps `?? new Set()` from turning a typo into a pass.
  ok(bundled.has(UI_FAMILY.pkg), `\`fonts.css\` imports the UI family as \`${UI_FAMILY.pkg}\``)
  const uiWeights = bundled.get(UI_FAMILY.pkg)?.weights ?? new Set()
  const boldToken = resolveTheme(docRules, 'light', ['--w-bold'])['--w-bold']
  ok(
    /var\(--w-bold\)/.test(uaBold?.body ?? '') && uiWeights.has(Number(boldToken)),
    `\`--w-bold\` (${boldToken}) is a weight the UI family (${UI_FAMILY.pkg}) actually ships`,
  )

  // The backstop, and the line it replaced. `-webkit-font-smoothing` is implemented against
  // CoreGraphics on macOS only; on WebKitGTK it is parsed and dropped, so it was a rule that
  // could never have an effect sitting exactly where the next reader looks for one.
  const rootCss = readFileSync('src/styles/tokens.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
  ok(/font-synthesis:\s*none/.test(rootCss), '`font-synthesis: none` still forbids a faked face')
  ok(
    !/-webkit-font-smoothing/.test(rootCss),
    '`-webkit-font-smoothing` has not come back — it does nothing on this engine',
  )

  // Every token in the mono scale has a reader. `--lh-code` shipped without one: declared,
  // documented as "what CSS uses", and read by no rule, while the editor restated `21px`.
  const moduleText = cssModules('src')
    .map((f) => readFileSync(f, 'utf8'))
    .join('\n')
  const xtermText = readFileSync('src/terminal/xterm.ts', 'utf8')
  for (const token of ['--fs-code', '--lh-code']) {
    ok(
      moduleText.includes(`var(${token})`),
      `\`${token}\` is read by a stylesheet rather than restated as a literal beside it`,
    )
  }
  // The terminal's two, matched on the *call* rather than on the token name appearing
  // anywhere in the file. The old assertion was `xtermText.includes('--fs-code')`, which a
  // comment satisfies — and did: this file kept passing after the terminal was moved onto
  // `--fs-term`, because the paragraph explaining the move still named the old token.
  for (const token of ['--fs-term', '--term-line-height']) {
    ok(
      new RegExp(`metric\\(style, '${token}'`).test(xtermText),
      `the terminal reads \`${token}\` rather than a number of its own`,
    )
  }

  // The runtime half of the floor above, asserted on the *option assignment* rather than on the
  // constant appearing somewhere in the file — the lesson two paragraphs up, where a comment
  // naming a token kept a check green after the code had moved off it. Without this line the
  // palette gate is the only thing standing, and it cannot see a background a program paints
  // for itself: Claude Code's default theme prints `rgb(255,255,255)` as a 24-bit literal, which
  // reaches no ANSI slot and lands at 1.00:1 on this app's white terminal.
  ok(
    /minimumContrastRatio:\s*TERMINAL_MIN_CONTRAST/.test(xtermText),
    '`minimumContrastRatio` is handed the same floor this file gates the palette with, so a ' +
      'foreground a program pairs with its own background is still readable',
  )

  // And that a size change reaches a terminal that already exists. The editor follows the
  // cascade; a terminal holds resolved numbers, so without this call the two Settings
  // controls move the editor and leave every open pane where it was — which is most of what
  // "the setting does nothing" looked like.
  ok(/export function refont\(/.test(xtermText), '`refont` exists to repaint live terminals')
  const appliesFonts = readFileSync('src/settings/useSettings.ts', 'utf8')
  ok(
    /refont\(/.test(appliesFonts),
    '`refont` is called when the settings change — otherwise only new panes pick the size up',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `theme: ok (${themeRules.length} palette rules, ${TERMINAL_TOKENS.length} terminal tokens, ` +
      `${referenced.size} referenced by stylesheets, ` +
      `${[...bundled.values()].reduce((n, f) => n + f.weights.size + f.italics.size, 0)} faces)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

/**
 * Every declaration block in the stylesheet that contains no nested block.
 *
 * Leaf blocks rather than top-level ones so an `@media` or `@supports` wrapper — which
 * `tokens.css` does not have today and might grow — yields the rule inside it rather than
 * one unparseable lump. A full CSS parser would be the alternative; this file is 140 lines
 * of custom properties and a dependency for it is not worth carrying.
 */
function leafRules(css) {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '')
  const rules = []
  const opens = []
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '{') opens.push(i)
    else if (text[i] === '}') {
      const open = opens.pop()
      if (open === undefined) continue
      const body = text.slice(open + 1, i)
      if (body.includes('{')) continue
      let j = open - 1
      while (j >= 0 && text[j] !== '{' && text[j] !== '}') j--
      rules.push({ selector: text.slice(j + 1, open).trim().replace(/\s+/g, ' '), body })
    }
  }
  return rules
}

// Declarations, not `const` arrows: the assertions above run before this point in the file
// and only a function declaration is hoisted into them.
function declares(body, token) {
  return new RegExp(`(^|[;{\\s])${token}\\s*:`).test(body)
}

function parts(selector) {
  return selector.split(',').map((s) => s.trim())
}

/** `:root`, `html`, `[data-theme='x']` and combinations — never a descendant of one. */
function themeLevel(selector) {
  return parts(selector).every((p) => /^(:root|html|\[data-theme=['"]?[\w-]+['"]?\])+$/.test(p))
}

function isBase(selector) {
  return parts(selector).some((p) => /^(:root|html)$/.test(p))
}

function themeNames(selector) {
  return [...selector.matchAll(/\[data-theme=['"]?([\w-]+)['"]?\]/g)].map((m) => m[1])
}

/** Base declarations first, then the ones this theme overrides — cascade order. */
function resolveTheme(themeRules, theme, tokens) {
  const values = {}
  const apply = (rule) => {
    for (const token of tokens) {
      const match = rule.body.match(new RegExp(`(?:^|[;{\\s])${token}\\s*:([^;]+)`))
      if (match) values[token] = match[1].trim()
    }
  }
  for (const rule of themeRules) if (isBase(rule.selector)) apply(rule)
  for (const rule of themeRules) if (themeNames(rule.selector).includes(theme)) apply(rule)
  return values
}

/** Every `*.module.css` under `dir`, recursively. */
function cssModules(dir) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...cssModules(path))
    else if (entry.name.endsWith('.module.css')) out.push(path)
  }
  return out
}

/**
 * WCAG contrast ratio between two `#rgb`/`#rrggbb` colours, or `null` if either is not one.
 *
 * `null` rather than a throw or a 1: a token legitimately holding an `rgba()` or a gradient is
 * not a failure, it is simply not something this ratio is defined for. Returning 1 would
 * report every such token as invisible and train the reader to ignore this check.
 */
function contrast(a, b) {
  const la = luminance(a)
  const lb = luminance(b)
  if (la === null || lb === null) return null
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

/** Relative luminance per WCAG 2.x, or `null` for anything that is not a hex colour. */
function luminance(colour) {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec((colour ?? '').trim())
  if (!hex) return null
  const digits =
    hex[1].length === 3
      ? [...hex[1]].map((c) => c + c)
      : [hex[1].slice(0, 2), hex[1].slice(2, 4), hex[1].slice(4, 6)]
  const [r, g, b] = digits
    .map((d) => parseInt(d, 16) / 255)
    .map((c) => (c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)))
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}
