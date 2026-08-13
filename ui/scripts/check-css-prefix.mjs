/**
 * Every CSS property this app's engine only knows under a `-webkit-` name is written twice.
 *
 * # The bug this exists to prevent, which had already happened twice
 *
 * The git changes tree text-selected on drag. Two people fixed it, each by adding
 * `user-select: none` — one on the app root in `styles/tokens.css`, one on the tree in
 * `sidebar/GitPanel/ChangesTree.module.css`, the second with a comment naming the exact
 * symptom. Both rules were correct CSS, both were in the right place, and neither did
 * anything, because **WebKitGTK 2.52.3 does not implement unprefixed `user-select`**. Its
 * parser drops the declaration: the rule comes back out of the CSSOM with the property
 * missing from `style`, `CSS.supports('user-select', 'none')` is `false`, and
 * `getComputedStyle(el).userSelect` is `undefined`.
 *
 * What made it survive two attempts is that it is invisible in a release build.
 * `vite.config.ts` sets `build.target: 'safari18'` and esbuild lowers the property to the
 * prefixed form — but only in `vite build`. `./run.sh`, the documented way to run this app,
 * serves `ui/src` straight off the dev server, which lowers nothing. So the bug reproduces on
 * exactly the launch path a developer uses and disappears on the one they ship, and each
 * author verified their fix on whichever half they happened to try.
 *
 * # Why a gate rather than a build-time transform
 *
 * `css.transformer: 'lightningcss'` would lower in dev *and* in build and would end the class
 * outright. It also pins a new dependency and moves a correctness property into a bundler
 * setting, where the next person reading the stylesheet sees a bare `user-select` and has no
 * way to know it works. Written twice in the source, the stylesheet says what it means. This
 * script is what keeps the pair together.
 *
 * # The list
 *
 * One member, and it earned its place experimentally rather than by reputation: an offscreen
 * WebKit2 4.1 webview on this machine was asked about fourteen properties this app uses —
 * `appearance`, `clip-path`, `backdrop-filter`, `mask-image`, `color-mix()`, `font-synthesis`,
 * `position: sticky`, `inset`, `gap`, `text-wrap`, `scrollbar-width`, `aspect-ratio` and both
 * spellings of `user-select`. `user-select` was the only unprefixed failure. Do not add
 * entries here on the strength of a compatibility table; add them when a probe says so, and
 * say in the entry what the probe found.
 *
 * # The direction that is easy to get backwards
 *
 * The `none` declarations are not the only ones that have to be prefixed. Five surfaces opt
 * *back in* with `user-select: text` so their contents can be copied out — the editor, both
 * diff panes, the problems explainer, the commit message box. Prefixing the `none` on the app
 * root without prefixing those turns them from accidentally-selectable into genuinely
 * unselectable, because the prefixed `none` now inherits and the unprefixed opt-in is still
 * being dropped. That is a worse app than the one before the fix. So this checks *every*
 * declaration of a listed property, whatever its value.
 *
 * # And the CSSOM half
 *
 * `el.style.userSelect = 'none'` is the trap with teeth: in this engine it **reads back as
 * `'none'`** while `style.cssText` stays empty and nothing changes on screen. So the property
 * may not be assigned through the CSSOM anywhere except `chrome/dragLock.ts`, which writes
 * both spellings through `setProperty` and explains itself.
 *
 * Run: `pnpm --dir ui run check:css-prefix`
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

const UI = resolve(import.meta.dirname, '..')
const SRC = join(UI, 'src')

/**
 * Properties WebKitGTK 2.52.3 only accepts under `-webkit-`, with what the probe found.
 *
 * `cssom` names the module allowed to write the property imperatively — everywhere else must
 * go through it, or through a stylesheet.
 */
const PREFIXED = [
  {
    property: 'user-select',
    js: 'userSelect',
    cssom: 'src/chrome/dragLock.ts',
    probe:
      "CSS.supports('user-select','none') === false; the declaration is absent from the "
      + 'parsed rule and from getComputedStyle',
  },
]

let failed = 0
const fail = (what) => {
  console.error(`FAIL ${what}`)
  failed++
}

/** Every file under `src/` with one of these extensions, in a stable order. */
function walk(dir, exts) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
    a.name < b.name ? -1 : 1,
  )) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...walk(path, exts))
    else if (exts.some((ext) => entry.name.endsWith(ext))) out.push(path)
  }
  return out
}

const css = walk(SRC, ['.css'])
const code = walk(SRC, ['.ts', '.tsx'])

if (css.length === 0) fail('no stylesheets found under src/ — this check is looking in the wrong place')

// --- every declaration is written twice, prefixed first --------------------------------

/*
 * Line-based rather than through a CSS parser, and that is a deliberate limit. The rule this
 * enforces is "the two spellings sit on adjacent lines, prefixed first", which is stricter
 * than "both are somewhere in the same block" and is checkable by reading. A parser would
 * accept the pair split across a rule by twenty lines of other declarations, which is a
 * stylesheet the next person edits by moving one and not the other.
 */
for (const { property, probe } of PREFIXED) {
  const declaration = new RegExp(`^(\\s*)(-webkit-)?${property}\\s*:\\s*([^;]+);\\s*$`)
  for (const file of css) {
    const lines = readFileSync(file, 'utf8').split('\n')
    const where = relative(UI, file)
    lines.forEach((line, i) => {
      const m = declaration.exec(line)
      if (m === null || m[2] !== undefined) return
      const before = lines[i - 1] ?? ''
      const prev = declaration.exec(before)
      if (prev === null || prev[2] === undefined || prev[3].trim() !== m[3].trim()) {
        fail(
          `${where}:${i + 1} declares \`${property}: ${m[3].trim()}\` with no `
            + `\`-webkit-${property}: ${m[3].trim()}\` on the line above.\n`
            + `  This engine drops the unprefixed form (${probe}), so the declaration `
            + 'has no effect at all — in the dev server, which is what `./run.sh` uses.\n'
            + '  Write both, prefixed first. See `styles/tokens.css`.',
        )
      }
    })
  }
}

// --- and nothing writes it through the CSSOM behind `dragLock`'s back -------------------

for (const { property, js, cssom, probe } of PREFIXED) {
  const assignment = new RegExp(`\\.style\\.${js}\\s*=`)
  const setter = new RegExp(`setProperty\\(\\s*'${property}'`)
  for (const file of code) {
    const where = relative(UI, file).split('\\').join('/')
    if (where === cssom) continue
    const text = readFileSync(file, 'utf8')
    if (assignment.test(text) || setter.test(text)) {
      fail(
        `${where} writes \`${property}\` through the CSSOM. Only \`${cssom}\` may.\n`
          + `  ${probe} — and \`style.${js} = 'none'\` READS BACK as 'none' here while `
          + 'changing nothing, so the assignment looks like it worked.',
      )
    }
  }
}

// --- the module that is allowed to do it writes both spellings --------------------------

for (const { property, js, cssom } of PREFIXED) {
  const text = readFileSync(join(UI, cssom), 'utf8')
  if (!new RegExp(`setProperty\\(\\s*'-webkit-${property}'`).test(text)) {
    fail(`${cssom} is the one module allowed to set \`${property}\`, and it does not set the prefixed form`)
  }
  if (!new RegExp(`\\.style\\.${js}\\s*=`).test(text)) {
    fail(`${cssom} sets only the prefixed \`${property}\`; the standard spelling belongs there too`)
  }
}

if (failed > 0) {
  console.error(`\n${failed} problem(s)`)
  process.exit(1)
}
console.log(
  `ok — ${PREFIXED.map((p) => p.property).join(', ')} written twice in every one of `
    + `${css.length} stylesheets, and set imperatively only in ${PREFIXED[0].cssom}`,
)
