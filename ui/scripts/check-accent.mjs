/**
 * The user's accent colour reaches every accent token, and nothing that is not one.
 *
 * # Why this exists
 *
 * A chosen accent is two inline properties (`--accent-l`, `--accent-d`) plus
 * `data-accent="custom"` on <html>. `tokens.css`'s `[data-accent='custom']` blocks re-derive the
 * accent family from those two. Every way this goes wrong is silent:
 *
 *   1. A new accent token added to the theme blocks with no line in the custom blocks stays red
 *      under a blue accent. Nothing errors, and a user who picked blue sees a red element they
 *      cannot account for.
 *   2. A status or danger token (`--red`, `--grad-red`, `--grad-danger`) pulled into a custom
 *      block would repaint "failed" or "destroy" in the brand colour. That is exactly the
 *      confusion the black danger fill was introduced to end.
 *   3. `public/theme-boot.js` and `useSettings.ts`'s `paintAccent` write the same attribute and
 *      properties from two places that cannot import each other. If one is renamed, the first
 *      frame paints the red default and the app flips to the chosen colour a moment later, or
 *      the reverse.
 *   4. The presets Settings offers are also listed in the Rust test that proves each one fits.
 *      A preset added on one side only is either an untested button or a tested colour nobody
 *      can pick.
 *
 * Source is read with comments stripped: the prose around these blocks names every token it
 * warns about.
 *
 * Run: `pnpm --dir ui run check:accent`
 */
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

const uncomment = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`])\/\/.*$/gm, '$1')

const tokens = uncomment(readFileSync(resolve('src/styles/tokens.css'), 'utf8'))

/** The body of the first rule whose selector is exactly `selector`. */
function block(selector) {
  const at = tokens.indexOf(`${selector} {`)
  if (at < 0) return null
  const open = tokens.indexOf('{', at)
  let depth = 0
  for (let i = open; i < tokens.length; i++) {
    if (tokens[i] === '{') depth++
    else if (tokens[i] === '}' && --depth === 0) return tokens.slice(open + 1, i)
  }
  return null
}

/** `--name` → value, for the top-level declarations of a block (multi-line values joined). */
function declarations(body) {
  const out = new Map()
  for (const m of body.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) out.set(m[1], m[2].replace(/\s+/g, ' ').trim())
  return out
}

const light = block(":root,\n[data-theme='light']")
const dark = block("[data-theme='dark']")
const customLight = block(":root[data-accent='custom']")
const customDark = block(":root[data-accent='custom'][data-theme='dark']")
ok(light !== null && dark !== null, 'the two theme blocks are where this check looks for them')
ok(customLight !== null, "tokens.css has the `:root[data-accent='custom']` block")
ok(customDark !== null, "tokens.css has the `:root[data-accent='custom'][data-theme='dark']` block")

// --- 1: every accent-family token is re-derived --------------------------------------------

/**
 * The accent family, by name. Named rather than detected by colour because the family is a
 * decision (`--sel` is pink *because* the accent is red), and a detector keyed on hue would
 * miss the next one exactly when it matters. `--on-accent` is absent on purpose: it is white
 * in both themes, and `fit` guarantees the ink it sits on is dark enough for white.
 */
const ACCENT_TOKENS = [
  '--accent',
  '--accent-hi',
  '--accent-dim',
  '--sel',
  '--grad-from',
  '--grad-to',
  '--grad-accent-ink',
  '--grad-accent-soft',
]

/** The shipped red's literals. Any theme-block token that carries one of them is family. */
const RED_LITERALS = [
  /#dc1f2b/i, /#ff5a5f/i, /#b3121d/i, /#ff8a8d/i, /#f7c9cc/i, /#6b1f24/i,
  /#ffe4e6/i, /#3b1c21/i, /#ff3b3b/i, /#ff4d55/i, /#e11d48/i,
  /rgba\(\s*224,\s*32,\s*42/, /rgba\(\s*255,\s*77,\s*85/, /rgba\(\s*225,\s*29,\s*72/,
]
/** Status and danger: they share literals with the accent and must never follow it. */
const NEVER = ['--red', '--grad-red', '--grad-danger', '--on-danger', '--danger-edge']

for (const [name, theme, custom] of [
  ['light', light, customLight],
  ['dark', dark, customDark],
]) {
  if (theme === null || custom === null) continue
  const themed = declarations(theme)
  const derived = declarations(custom)
  const family = new Set(ACCENT_TOKENS)
  for (const [token, value] of themed) {
    if (!NEVER.includes(token) && !token.startsWith('--tk-') && RED_LITERALS.some((re) => re.test(value))) family.add(token)
  }
  for (const token of family) {
    ok(themed.has(token), `${token} is still declared in the ${name} theme block (or drop it from ACCENT_TOKENS)`)
    ok(
      derived.has(token),
      `${token} is accent-family in the ${name} theme but has no line in its [data-accent='custom'] block — it would stay red under a chosen accent`,
    )
  }
  for (const token of NEVER) {
    ok(!derived.has(token), `${token} is status/danger and must not follow the accent (${name} custom block)`)
  }
  for (const [token, value] of derived) {
    ok(
      /var\(--accent-[ld]\)/.test(value),
      `${token} in the ${name} custom block derives from --accent-l/--accent-d (got ${value})`,
    )
  }
}

// --- 3: the first frame and the live paint agree -------------------------------------------

const boot = uncomment(readFileSync(resolve('public/theme-boot.js'), 'utf8'))
const paint = uncomment(readFileSync(resolve('src/settings/useSettings.ts'), 'utf8'))
for (const name of ['--accent-l', '--accent-d']) {
  ok(boot.includes(`'${name}'`), `theme-boot.js writes ${name}`)
  ok(paint.includes(`'${name}'`), `useSettings.ts paintAccent writes ${name}`)
  ok(tokens.includes(`var(${name})`), `tokens.css reads ${name}`)
}
ok(/dataset\.accent\s*=\s*'custom'/.test(boot), "theme-boot.js sets data-accent='custom'")
ok(/dataset\.accent\s*=\s*'custom'/.test(paint), "paintAccent sets data-accent='custom'")
ok(/delete root\.dataset\.accent/.test(paint), 'paintAccent removes data-accent when the accent is reset')
ok(
  boot.includes('/^([0-9a-f]{6}),([0-9a-f]{6})$/'),
  'theme-boot.js accepts only two bare lowercase hexes — windows.rs::accent_param writes exactly that',
)
const windows = readFileSync(resolve('../crates/cide-app/src/windows.rs'), 'utf8')
ok(windows.includes('"&accent={light},{dark}"'), 'windows.rs puts `&accent=light,dark` on the URL theme-boot.js reads')

// --- 4: the presets Settings offers are the ones Rust proves fit ---------------------------

const presetsTs = readFileSync(resolve('src/settings/accentPresets.ts'), 'utf8')
const rust = readFileSync(resolve('../crates/cide-core/src/accent.rs'), 'utf8')
const offered = [...presetsTs.matchAll(/value: '(#[0-9a-f]{6})'/g)].map((m) => m[1])
const defaultHex = /DEFAULT_ACCENT = '(#[0-9a-f]{6})'/.exec(presetsTs)?.[1]
const rustBlock = /const PRESETS: &\[&str\] = &\[([\s\S]*?)\];/.exec(rust)?.[1] ?? ''
const proven = [...rustBlock.matchAll(/"(#[0-9a-f]{6})"/g)].map((m) => m[1])
ok(offered.length >= 4, `accentPresets.ts offers presets (found ${offered.length})`)
ok(
  JSON.stringify([defaultHex, ...offered].sort()) === JSON.stringify([...proven].sort()),
  `accentPresets.ts (${[defaultHex, ...offered].join(' ')}) and accent.rs's PRESETS (${proven.join(' ')}) list the same colours`,
)
ok(/--accent: #dc1f2b;/.test(light ?? '') && defaultHex === '#dc1f2b', "DEFAULT_ACCENT is the light theme's shipped --accent")

if (failed > 0) {
  console.error(`\ncheck:accent: ${failed} failure(s)`)
  process.exit(1)
}
console.log(`accent: ${ACCENT_TOKENS.length} tokens re-derived in both themes, boot and paint agree, presets proven`)
