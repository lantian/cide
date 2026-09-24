/**
 * The fence around the UI kit (`ui/src/kit/`, `docs/ui-kit.md`).
 *
 * # Why this exists
 *
 * The kit is only worth anything while three things agree: the components, the page that shows
 * them, and the document that says when to use which. Each drifts silently on its own. A
 * component added for one feature and never given a specimen is invisible to the next person
 * looking for one, so they write a second; a component missing from `docs/ui-kit.md` is invisible
 * to Claude, who reads the document and not the page. Before the kit, the app reached ~45
 * separately styled secondary buttons exactly this way.
 *
 * # What it asserts
 *
 *   1. **Every exported component has a specimen** — it is used by a chapter under
 *      `kit/page/`, by `Kit.tsx`, or by another kit component (the way `Wizard` draws `Stepper`).
 *   2. **Every exported component is named in `docs/ui-kit.md`**, in backticks.
 *   3. **Every chapter in `Kit.tsx`'s `CHAPTERS` is listed in the document** by its id.
 *   4. **The kit is standalone**: nothing under `src/kit/` imports outside React, `@/icons/Icon`,
 *      `@/icons/iconPaths`, `src/styles/` and the kit itself. The page runs in a plain browser
 *      tab with no Tauri behind it; an `ipc/` import there would throw on load, and in a
 *      component it would tie a reusable part to one panel's module graph.
 *   5. **The page renders** under node — every chapter's anchor is in the HTML, and no class
 *      attribute contains `undefined` (a CSS-module key that does not exist).
 *
 * Source is read with comments stripped: house-style comments name the thing they warn about,
 * so the needle would otherwise be found in the prose.
 *
 * Run: `pnpm --dir ui run check:kit`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

const KIT = resolve('src/kit')
const DOC = readFileSync(resolve('../docs/ui-kit.md'), 'utf8')

/** Line and block comments out, strings kept. Good enough for our own sources. */
const uncomment = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`])\/\/.*$/gm, '$1')

function walk(dir) {
  const found = []
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) found.push(...walk(p))
    else if (/\.(tsx?|css)$/.test(e.name)) found.push(p)
  }
  return found.sort()
}

const files = walk(KIT)
const source = Object.fromEntries(files.map((f) => [f, uncomment(readFileSync(f, 'utf8'))]))
const componentFiles = files.filter((f) => f.startsWith(join(KIT, 'components')) && f.endsWith('.tsx'))

// --- 1 and 2: every exported component has a specimen and a doc entry -----------------------

const exported = []
for (const f of componentFiles) {
  for (const m of source[f].matchAll(/export function ([A-Z]\w*)/g)) exported.push({ name: m[1], file: f })
}
ok(exported.length > 20, `the kit exports its components (found ${exported.length})`)

for (const { name, file } of exported) {
  // Its own file counts: `Wizard` drawing `Stepper` beside it is a specimen of `Stepper`. The
  // definition itself is `function Name(`, which the `<Name` needle does not match.
  const users = files.filter(
    (f) => f.endsWith('.tsx') && new RegExp(`<${name}[\\s/>]`).test(source[f]),
  )
  ok(
    users.length > 0,
    `${name} (${relative(KIT, file)}) is drawn somewhere on the kit page — give it a specimen in a chapter under kit/page/chapters/`,
  )
  ok(
    DOC.includes(`\`${name}\``),
    `${name} (${relative(KIT, file)}) is named in docs/ui-kit.md — add its entry to the component section`,
  )
}

// --- 3: every chapter is in the document ----------------------------------------------------

const kitSrc = source[join(KIT, 'Kit.tsx')]
const chapterIds = [...kitSrc.matchAll(/id: '([a-z-]+)'/g)].map((m) => m[1])
ok(chapterIds.length >= 10, `Kit.tsx declares its chapters (found ${chapterIds.length})`)
for (const id of chapterIds) {
  ok(DOC.includes(`\`${id}\``), `chapter \`${id}\` is listed in docs/ui-kit.md's chapter table`)
}

// --- 4: the kit is standalone ----------------------------------------------------------------

const ALLOWED = [
  /^react$/,
  /^react\/jsx-runtime$/,
  /^react-dom$/,
  /^react-dom\/client$/,
  /^react-dom\/server$/,
  /^@\/icons\/Icon$/,
  /^@\/icons\/iconPaths$/,
  /^\.\.?\//,
]
for (const f of files.filter((f) => /\.tsx?$/.test(f))) {
  for (const m of source[f].matchAll(/^\s*import\s[^'"]*?['"]([^'"]+)['"]/gm)) {
    const spec = m[1]
    const allowed = ALLOWED.some((re) => re.test(spec))
    ok(allowed, `${relative(KIT, f)} imports only React, the icon component, styles and the kit (found '${spec}')`)
    if (allowed && spec.startsWith('.')) {
      const target = resolve(join(f, '..'), spec)
      ok(
        target.startsWith(KIT)
          || target.startsWith(resolve('src/styles'))
          || target === resolve('src/icons/icon.css'),
        `${relative(KIT, f)}'s relative import stays inside kit/, styles/ or the icon box (found '${spec}')`,
      )
    }
  }
}

// --- 5: the page renders ----------------------------------------------------------------------

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-kit-render-')
try {
  execFileSync(
    'node',
    ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/kit/smokeEntry.tsx', '--outDir', out, '--logLevel', 'error'],
    { stdio: 'inherit' },
  )
  const printed = []
  const log = console.log
  try {
    console.log = (line) => printed.push(line)
    await import(`file://${resolve(out, 'smokeEntry.js')}`)
  } finally {
    console.log = log
  }
  const { html, chapters } = JSON.parse(printed.at(-1))
  ok(chapters.length === chapterIds.length, 'the rendered chapter list matches Kit.tsx')
  for (const id of chapters) ok(html.includes(`id="${id}"`), `chapter #${id} renders`)
  ok(!/class="[^"]*undefined/.test(html), 'no class attribute names a CSS-module key that does not exist')
  // The page is long; a render that stopped early (a chapter throwing inside a boundary-less
  // tree aborts the whole string) would be far shorter than this.
  ok(html.length > 50_000, `the page rendered in full (${html.length} chars)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`${failed} kit check(s) failed`)
  process.exit(1)
}
console.log(`UI kit: ${exported.length} components, ${chapterIds.length} chapters, all drawn, documented and standalone`)
