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
  // 3:1 rather than AA's 4.5:1 for body text. Terminal colours are not body text, so the floor
  // is set where "a human cannot see this at all" lives, not where "comfortable" does.
  //
  // The exemptions are per theme, and that asymmetry IS the rule rather than a softening of
  // it. A dark scheme is conventionally expected to sink colour 0 into its background — that
  // is what makes it usable as a shadow and a fill — so `black` at 1.06:1 on #151518 is
  // correct there and would be a bug in a white terminal. `brightBlack` is the dim-grey slot
  // by universal convention and is only ever asked to be *quiet*, not invisible; it is
  // exempted in dark alone, and clears the floor unaided in light.
  //
  // The consequence worth stating: `black` is checked in light, which is precisely the bug
  // this was written after. Move colour 0 back to a near-background token and the light case
  // fails while dark stays green.
  const INK_FLOOR = 3
  const CONVENTIONALLY_QUIET = { dark: ['black', 'brightBlack'], light: [] }
  const inkSlots = TERMINAL_SLOTS.filter(
    ([slot]) => !['background', 'cursorAccent', 'selectionBackground'].includes(slot),
  )
  for (const [name, palette] of [['dark', dark], ['light', light]]) {
    const ground = palette['--panel']
    const invisible = inkSlots
      .filter(([slot]) => !CONVENTIONALLY_QUIET[name].includes(slot))
      .map(([slot, token]) => [slot, palette[token], contrast(palette[token], ground)])
      .filter(([, , ratio]) => ratio !== null && ratio < INK_FLOOR)
      .map(([slot, value, ratio]) => `${slot}=${value} ${ratio.toFixed(2)}:1`)
    eq(
      invisible,
      [],
      `every ink colour clears ${INK_FLOOR}:1 on the ${name} terminal background ${ground}`,
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

  const wanted = [...referenced.keys()].sort()
  const darkAll = resolveTheme(docRules, 'dark', wanted)
  const lightAll = resolveTheme(docRules, 'light', wanted)
  for (const theme of [['dark', darkAll], ['light', lightAll]]) {
    eq(
      wanted.filter((t) => (theme[1][t] ?? '') === '').map((t) => `${t} (${referenced.get(t)})`),
      [],
      `every token the stylesheets use resolves under ${theme[0]} — a missing one takes its ` +
        `\`var()\` fallback in silence`,
    )
  }

  // --- typography: never ask the engine for a face this app did not bundle ----------------
  //
  // Same class of bug as the palette above, and the same reason it lives in a script: it
  // fails *silently*. A stylesheet asking for a weight or a style with no bundled face does
  // not error — the engine picks the nearest real face and, unless `font-synthesis` says
  // otherwise, emboldens or shears it. That is the "fonts are ugly" report: `<h3>`, `<b>` and
  // `<strong>` were being fake-bolded out of the 600 face because the UA sheet asks for 700
  // and IBM Plex Sans ships 400/500/600, and five surfaces asking `font-style: italic` were
  // getting an obliqued upright because no italic was imported at all.
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
  // assumed: everything below reads `--font-ui`/`--font-mono` as "the IBM Plex Sans stack"
  // and "the JetBrains Mono stack", and a swapped stack would make every check here vacuous.
  const stacks = resolveTheme(docRules, 'light', ['--font-ui', '--font-mono'])
  ok(
    (stacks['--font-ui'] ?? '').startsWith("'IBM Plex Sans'"),
    '`--font-ui` still leads with the family `fonts.css` imports as `ibm-plex-sans`',
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
  const uiWeights = bundled.get('ibm-plex-sans')?.weights ?? new Set()
  const boldToken = resolveTheme(docRules, 'light', ['--w-bold'])['--w-bold']
  ok(
    /var\(--w-bold\)/.test(uaBold?.body ?? '') && uiWeights.has(Number(boldToken)),
    `\`--w-bold\` (${boldToken}) is a weight the UI family actually ships`,
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
