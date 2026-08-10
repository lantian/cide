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
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
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
  eq(themeToAdopt('dark', 'light'), 'light', 'a window adopts the theme another window set')
  eq(themeToAdopt('light', 'dark'), 'dark', 'and in the other direction')
  eq(
    themeToAdopt('light', 'light'),
    null,
    'the window that made the change has nothing to adopt — this runs inside a store ' +
      'subscription, and answering with a value here would write the store on every snapshot',
  )
  eq(
    themeToAdopt('light', undefined),
    null,
    'a window whose mirror has no settings yet has not been told anything and must not ' +
      'overwrite what it is showing',
  )

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
  const dark = resolveTheme(themeRules, 'dark', TERMINAL_TOKENS)
  const light = resolveTheme(themeRules, 'light', TERMINAL_TOKENS)
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

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log(`theme: ok (${themeRules.length} palette rules, ${TERMINAL_TOKENS.length} tokens)`)
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
