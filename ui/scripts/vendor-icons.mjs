/**
 * Re-vendors `public/icons/` and regenerates `src/icons/iconMap.ts` from upstream.
 *
 * Maintenance only — this is not part of the build, and it needs network access. Run it to
 * pick up new upstream associations, or after editing {@link FILE_ICONS} / {@link FOLDER_ICONS}
 * below, which are the whole of "which icons does this project ship":
 *
 *     node ui/scripts/vendor-icons.mjs        # from the repository root
 *     pnpm --dir ui run check:icons           # then always this
 *
 * Source: https://github.com/material-extensions/vscode-material-icon-theme, MIT. The licence
 * is copied to `public/icons/LICENSE` by this script so it travels with the artwork.
 *
 * **Why a script rather than 190 hand-copied files.** The associations are the valuable part
 * and they are not obvious: `.tsx` is `react_ts` and not `typescript`, `vite.config.ts` exists
 * only because upstream's `vite` entry carries `patterns: { vite: Ecmascript }`, and
 * `package.json` beats `.json` only because whole names outrank extensions. Transcribing that
 * by hand is how a set drifts into looking *almost* like the icon theme people know. So this
 * reads upstream's own `src/core/icons/*.ts` and replays upstream's own resolution:
 *
 *   * `parseByPattern` is imported verbatim out of the upstream checkout rather than
 *     reimplemented, so pattern expansion cannot drift;
 *   * `mapSpecificFileIcons` writes into a flat manifest in source order, so LAST wins;
 *   * `disableIconsByPack` drops `enabledFor` icons outside the active pack.
 *
 * Two things are deliberately *not* upstream's: folder-name decoration, which is undone in
 * `iconFor.ts` instead of multiplied into the table (see BY_FOLDER), and the light-theme
 * variants, which are extended by measurement to this project's white theme (see the light
 * pass near the bottom).
 */
import { readFileSync, writeFileSync, mkdirSync, rmSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'
import { tmpdir } from 'node:os'

const UI = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_ICONS = join(UI, 'public/icons')
const OUT_MAP = join(UI, 'src/icons/iconMap.ts')

const UPSTREAM = 'https://github.com/material-extensions/vscode-material-icon-theme.git'
/**
 * The revision `iconMap.ts`'s header names, and the one this script fetches.
 *
 * Pinned rather than `clone --depth 1` of whatever the default branch happens to be. The
 * header is a provenance claim — "these associations are upstream's, at this commit" — and a
 * script that records HEAD *after* cloning cannot reproduce the file it just stamped: the
 * next run silently re-vendors against a different upstream and rewrites 190 files under a
 * commit message that says nothing changed. Moving the set forward is a deliberate edit to
 * this line.
 *
 * `--depth 1` is kept, per revision: a shallow fetch of one commit, not of a branch tip.
 */
const UPSTREAM_REV = '5bcb461d8f1d3dc9b0f9e733720f776824380338'
/** Upstream's own default `activeIconPack`, from `src/core/generator/config/defaultConfig.ts`. */
const ACTIVE_PACK = 'angular'

// ---------------------------------------------------------------------------------------
// Fetch upstream and read its icon definitions.
//
// The definition files are TypeScript modules with imports and type annotations, so they are
// stripped down to something Node can evaluate rather than parsed: three type annotations and
// the import block come off, upstream's real `parseByPattern` is prepended, and `IconPack` is
// replaced by its enum values. Evaluating upstream's data is the point — a regex over it would
// be the drift this script exists to prevent.
// ---------------------------------------------------------------------------------------

const MIT = mkdtemp('cide-icons-upstream-')
function mkdtemp(prefix) {
  const p = join(tmpdir(), prefix + Math.random().toString(36).slice(2, 10))
  mkdirSync(p, { recursive: true })
  return p
}

const git = (...args) => execFileSync('git', args, { cwd: MIT, encoding: 'utf8' })

git('init', '--quiet')
git('remote', 'add', 'origin', UPSTREAM)
// `fetch <sha>` rather than `clone` + `checkout`: it moves one commit's worth of objects and
// fails loudly if the pin has been garbage-collected or force-pushed away, which is the case
// where a silent fallback to HEAD would be worst.
execFileSync('git', ['fetch', '--depth', '1', 'origin', UPSTREAM_REV], { cwd: MIT, stdio: 'inherit' })
git('checkout', '--quiet', 'FETCH_HEAD')
const upstreamRev = git('rev-parse', 'HEAD').trim()
if (upstreamRev !== UPSTREAM_REV) throw new Error(`fetched ${upstreamRev}, wanted ${UPSTREAM_REV}`)

const PATTERN_ENUM = `
const FileNamePattern = {
  Ecmascript: 'ecmascript', Configuration: 'configuration', NodeEcosystem: 'nodeEcosystem',
  Cosmiconfig: 'cosmiconfig', Yaml: 'yaml', Dotfile: 'dotfile',
};
const IconPack = {
  Angular: 'angular', Nest: 'nest', Ngrx: 'angular_ngrx', React: 'react', Redux: 'react_redux',
  Roblox: 'roblox', Qwik: 'qwik', Vue: 'vue', Vuex: 'vue_vuex', Bashly: 'bashly',
};
`

const noImports = (src) => src.replace(/^import .*?;$/gms, '')

/**
 * Upstream's `parseByPattern`, made runnable.
 *
 * Every edit below removes a *type*, and the assertions afterwards are what stops this
 * silently degrading into a no-op if upstream reformats the file — an un-expanded
 * `parseByPattern` would drop every pattern-derived name (`vite.config.ts`, `.eslintrc.json`,
 * every `.env.*`) and the only symptom would be a slightly duller file tree.
 */
const parseByPatternSrc = noImports(readFileSync(join(MIT, 'src/core/patterns/patterns.ts'), 'utf8'))
  .replace(': Patterns): string[]', ')')
  .replace(
    /export const parseByPattern = \(\s*rawFileIcons: FileIconWithPatterns\s*\): FileIcon\[\] => \{/,
    'const parseByPattern = (rawFileIcons) => {',
  )
  .replace(
    /\s*const exhaustiveCheck: never = pattern;\n\s*throw new Error\([^;]*\);/,
    '\n        throw new Error(`Unhandled pattern: ${pattern}`);',
  )

for (const needle of ['const parseByPattern = (rawFileIcons) => {', 'const mapPatterns = (patterns) => {']) {
  if (!parseByPatternSrc.includes(needle)) {
    throw new Error(`upstream's patterns.ts no longer matches: expected to produce \`${needle}\``)
  }
}

const prelude = `${PATTERN_ENUM}${parseByPatternSrc}\n`

/** Evaluate one upstream definition module: strip its imports and its one type annotation. */
const evalUpstream = async (relPath, typeAnnotation) => {
  const src = noImports(readFileSync(join(MIT, relPath), 'utf8')).replace(typeAnnotation, ' =')
  const out = join(MIT, `${relPath.replace(/[/.]/g, '_')}.mjs`)
  writeFileSync(out, prelude + src)
  return import(`file://${out}`)
}

const { fileIcons } = await evalUpstream('src/core/icons/fileIcons.ts', /:\s*FileIcons\s*=/)
const { folderIcons } = await evalUpstream('src/core/icons/folderIcons.ts', /:\s*FolderTheme\[\]\s*=/)
const U = { fileIcons, folderIcons }

if (!Array.isArray(U.fileIcons?.icons) || U.fileIcons.icons.length < 400) {
  throw new Error('upstream fileIcons did not evaluate to the expected shape')
}
if (!U.fileIcons.icons.some((i) => (i.fileNames ?? []).includes('vite.config.ts'))) {
  throw new Error('parseByPattern did not expand — see the note above `parseByPatternSrc`')
}

// ---------------------------------------------------------------------------------------
// What we vendor.
//
// Upstream ships ~1,500 icons. Shipping all of them would be ~1.2 MB of SVG for a set whose
// long tail is Aurelia, Roblox and Adobe Illustrator — so this is the languages and config
// files that actually appear in this repository, plus what an ordinary Rust/TS/web checkout
// drags in. Anything else lands on `file` / `folder`, which is a legible answer rather than
// an empty one; adding a name here and re-running is the whole cost of extending the set.
// ---------------------------------------------------------------------------------------

const FILE_ICONS = [
  // languages in this repo
  'rust', 'typescript', 'typescript-def', 'react_ts', 'react', 'javascript', 'test-ts',
  // markup / data / style
  'json', 'yaml', 'toml', 'xml', 'html', 'css', 'sass', 'markdown', 'mdx', 'graphql',
  // other languages an ordinary Rust/TS/web checkout drags in
  'python', 'go', 'c', 'cpp', 'h', 'hpp', 'console', 'vue', 'svelte', 'webassembly',
  // assets & opaque blobs
  'svg', 'image', 'font', 'zip', 'pdf', 'database', 'document', 'log', 'lock', 'diff',
  // toolchain & config, matched by file name
  'nodejs', 'npm', 'pnpm', 'yarn', 'tsconfig', 'vite', 'vitest', 'eslint', 'prettier',
  'biome', 'editorconfig', 'git', 'docker', 'makefile', 'just', 'tauri',
  'settings', 'tune', 'license', 'readme', 'changelog', 'contributing',
]

const FOLDER_ICONS = [
  'folder-src', 'folder-lib', 'folder-ui', 'folder-docs', 'folder-scripts', 'folder-contract',
  'folder-target', 'folder-dist', 'folder-node', 'folder-public', 'folder-images',
  'folder-test', 'folder-config', 'folder-github', 'folder-git', 'folder-vscode',
  'folder-rust', 'folder-css', 'folder-components', 'folder-hook', 'folder-utils',
  'folder-examples', 'folder-benchmark', 'folder-temp', 'folder-api', 'folder-typescript',
  'folder-resource', 'folder-log', 'folder-keys', 'folder-layout', 'folder-windows',
  'folder-coverage',
]

/** Upstream's `fileIcons.defaultIcon`, and the fallback `iconFor` names. */
const DEFAULT_FILE = 'file'

// ---------------------------------------------------------------------------------------
// Rebuild upstream's manifest maps.
// ---------------------------------------------------------------------------------------

const enabled = U.fileIcons.icons.filter(
  (i) => !i.enabledFor || i.enabledFor.some((p) => p === ACTIVE_PACK),
)

const extToIcon = new Map()
const nameToIcon = new Map()
for (const icon of enabled) {
  if (icon.disabled) continue
  for (const e of icon.fileExtensions ?? []) extToIcon.set(e.toLowerCase(), icon.name)
  for (const n of icon.fileNames ?? []) nameToIcon.set(n.toLowerCase(), icon.name)
}

// Upstream's `extendFolderNames` writes five keys per name — `x`, `.x`, `_x`, `-x`, `__x__` —
// because a VS Code manifest can only be a flat table. We keep the canonical name only and
// undo the decoration at lookup instead; five copies of 218 names is 870 rows of noise in a
// file a human has to be able to read.
const folderToIcon = new Map()
for (const theme of U.folderIcons) {
  if (theme.name !== 'specific') continue
  for (const icon of theme.icons ?? []) {
    if (icon.enabledFor && !icon.enabledFor.some((p) => p === ACTIVE_PACK)) continue
    for (const n of icon.folderNames ?? []) folderToIcon.set(n.toLowerCase(), icon.name)
  }
}

const lightIcons = new Set(
  [...U.fileIcons.icons, ...(U.folderIcons.find((t) => t.name === 'specific')?.icons ?? [])]
    .filter((i) => i.light)
    .map((i) => i.name),
)

// ---------------------------------------------------------------------------------------
// Invert onto the chosen set.
// ---------------------------------------------------------------------------------------

const chosenFiles = new Set(FILE_ICONS)
const chosenFolders = new Set(FOLDER_ICONS)

const invert = (map, chosen) => {
  const out = new Map()
  for (const [key, icon] of map) if (chosen.has(icon)) out.set(key, icon)
  return [...out].sort(([a], [b]) => (a < b ? -1 : 1))
}

const byExt = invert(extToIcon, chosenFiles)
const byName = invert(nameToIcon, chosenFiles)
const byFolder = invert(folderToIcon, chosenFolders)

// Sanity: an association we kept must not be unreachable because a *non*-chosen icon owns
// the key. `invert` already reads the winner, so this is only a report of coverage.
const unmatchedIcons = [...chosenFiles].filter(
  (n) => n !== DEFAULT_FILE && ![...byExt, ...byName].some(([, i]) => i === n),
)
const unmatchedFolders = [...chosenFolders].filter((n) => !byFolder.some(([, i]) => i === n))

// ---------------------------------------------------------------------------------------
// Copy / generate the SVGs.
// ---------------------------------------------------------------------------------------

const OPEN_PATH =
  'M14.483 6H4.721a1 1 0 0 0-.949.684L2 12V5h12a1 1 0 0 0-1-1H7.562a1 1 0 0 1-.64-.232l-.644-.536A1 1 0 0 0 5.638 3H2a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h11l2.403-5.606A1 1 0 0 0 14.483 6'
const CLOSED_PATH =
  'm6.922 3.768-.644-.536A1 1 0 0 0 5.638 3H2a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V5a1 1 0 0 0-1-1H7.562a1 1 0 0 1-.64-.232'
const FOLDER_COLOR = '#90a4ae'

rmSync(OUT_ICONS, { recursive: true, force: true })
mkdirSync(OUT_ICONS, { recursive: true })

const written = []
const copy = (name) => {
  const src = join(MIT, 'icons', `${name}.svg`)
  const body = readFileSync(src, 'utf8').trim()
  writeFileSync(join(OUT_ICONS, `${name}.svg`), `${body}\n`)
  written.push(`${name}.svg`)
}

/** Upstream's own `generateOpenFolderIcons`: swap the `#folder` path's `d`, keep the motive. */
const openVariant = (name) => {
  const src = readFileSync(join(MIT, 'icons', `${name}.svg`), 'utf8').trim()
  if (!src.includes('id="folder"')) throw new Error(`${name}: no id="folder"`)
  let seen = false
  const out = src.replace(/<path id="folder"([^>]*?)d="[^"]*"/, (m, attrs) => {
    seen = true
    return `<path id="folder"${attrs}d="${OPEN_PATH}"`
  })
  if (!seen) throw new Error(`${name}: could not rewrite the folder path`)
  writeFileSync(join(OUT_ICONS, `${name}-open.svg`), `${out}\n`)
  written.push(`${name}-open.svg`)
}

const svg = (inner) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">${inner}</svg>\n`

for (const n of FILE_ICONS) {
  copy(n)
  if (lightIcons.has(n)) copy(`${n}_light`)
}

// `file` is drawn by upstream's `generateFileIcons` at configure time, not shipped.
const FILE_PATH =
  'm8.668 6h3.6641l-3.6641-3.668v3.668m-4.668-4.668h5.332l4 4v8c0 0.73828-0.59375 1.3359-1.332 1.3359h-8c-0.73828 0-1.332-0.59766-1.332-1.3359v-10.664c0-0.74219 0.59375-1.3359 1.332-1.3359m3.332 1.3359h-3.332v10.664h8v-6h-4.668z'
writeFileSync(join(OUT_ICONS, 'file.svg'), svg(`<path fill="${FOLDER_COLOR}" d="${FILE_PATH}"/>`))
written.push('file.svg')

// No folder icon in our set has an upstream `_light` (only jinja / intellij / cursor do), so
// the open variant is generated from the closed one and the light pass below handles the rest.
for (const n of FOLDER_ICONS) {
  copy(n)
  openVariant(n)
}

// Drawn by upstream's `generateFolderIcons` at configure time rather than shipped, at the
// stock `#90a4ae` blue-grey.
//
// `folder-root` — upstream's target ring for a workspace root — is deliberately NOT vendored.
// `TreeRow.depth === 0` looks like the fact it needs and is not: the wire format documents
// depth 0 as "a project root when the project has several, the root's own children when it
// has one", so a single-root checkout would ring every top-level entry. Nothing else on the
// row carries the distinction, so drawing the ring would be a guess.
writeFileSync(join(OUT_ICONS, 'folder.svg'), svg(`<path fill="${FOLDER_COLOR}" d="${CLOSED_PATH}"/>`))
writeFileSync(join(OUT_ICONS, 'folder-open.svg'), svg(`<path fill="${FOLDER_COLOR}" d="${OPEN_PATH}"/>`))
written.push('folder.svg', 'folder-open.svg')

writeFileSync(join(OUT_ICONS, 'LICENSE'), readFileSync(join(MIT, 'LICENSE'), 'utf8'))

// ---------------------------------------------------------------------------------------
// Light-theme variants.
//
// The set is multi-colour by design and roughly a third of it is pitched for a dark editor:
// `editorconfig` is four near-whites (1.21:1 on #ffffff), `javascript` is amber #ffca28
// (1.53:1). On this project's light theme — which is *white*, not VS Code's #f3f3f3 — those
// rows are a filename with a smudge beside it.
//
// Upstream's own answer is a hand-drawn `_light.svg` for 49 icons; only two of ours have one.
// So the rule below extends upstream's mechanism to the rest instead of inventing a second
// one: scale every colour in the icon by a single factor in sRGB until its darkest colour
// clears 3:1 against white — WCAG 1.4.11's bar for a non-text graphic. A single factor,
// applied to all colours, is what keeps the icon recognisable: scaling all channels by k is
// exactly an HSV value change, so hue and saturation are untouched and the internal
// relationships (folder body vs. its pale motive) survive.
//
// Where upstream ships a `_light`, that file is the *input* to this pass rather than a
// competitor to it: fidelity where upstream's drawing suffices, darkening only if it does not.
// ---------------------------------------------------------------------------------------

const MIN_CONTRAST_ON_WHITE = 3.0

const srgbToLinear = (v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)
const luminance = (hex) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
  return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b)
}
const contrastOnWhite = (hex) => 1.05 / (luminance(hex) + 0.05)

const expand = (hex) =>
  hex.length === 4 ? `#${[...hex.slice(1)].map((c) => c + c).join('')}`.toLowerCase() : hex.toLowerCase()

const scale = (hex, k) =>
  `#${[1, 3, 5]
    .map((i) => Math.round(parseInt(hex.slice(i, i + 2), 16) * k))
    .map((v) => Math.max(0, Math.min(255, v)).toString(16).padStart(2, '0'))
    .join('')}`

const COLOUR_ATTR = /(fill|stroke|stop-color)="(#[0-9a-fA-F]{3}|#[0-9a-fA-F]{6})"/g

const coloursIn = (src) => [...src.matchAll(COLOUR_ATTR)].map((m) => expand(m[2]))

/** The largest k ≤ 1 for which every colour, scaled by k, is dark enough. Bisection: the
 *  contrast of a scaled colour is monotone in k, so 24 halvings is exact to ~1e-7. */
const darkenFactor = (colours) => {
  if (!colours.length) return 1
  const ok = (k) => Math.max(...colours.map((c) => contrastOnWhite(scale(c, k)))) >= MIN_CONTRAST_ON_WHITE
  if (ok(1)) return 1
  let lo = 0
  let hi = 1
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2
    if (ok(mid)) lo = mid
    else hi = mid
  }
  return lo
}

const generatedLight = []
for (const f of readdirSync(OUT_ICONS).filter((f) => f.endsWith('.svg') && !f.endsWith('_light.svg'))) {
  const stem = f.slice(0, -4)
  const upstreamLight = written.includes(`${stem}_light.svg`)
  const source = upstreamLight ? `${stem}_light.svg` : f
  const src = readFileSync(join(OUT_ICONS, source), 'utf8')
  const k = darkenFactor(coloursIn(src))
  if (k >= 1) {
    // Already legible on white. Upstream's own `_light`, if there is one, stays as drawn.
    continue
  }
  const out = src.replace(COLOUR_ATTR, (_m, attr, hex) => `${attr}="${scale(expand(hex), k)}"`)
  writeFileSync(join(OUT_ICONS, `${stem}_light.svg`), out)
  generatedLight.push([stem, k, upstreamLight])
}

// ---------------------------------------------------------------------------------------
// Emit the generated table.
// ---------------------------------------------------------------------------------------

const q = (s) => `'${s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`
const lit = (pairs, indent = '  ') =>
  pairs.map(([k, v]) => `${indent}${q(k)}: ${q(v)},`).join('\n')

// Read back off disk rather than recomputed: this list is a promise that a file exists, and
// the only thing that can keep that promise honest is the directory itself.
const lightNames = readdirSync(OUT_ICONS)
  .filter((f) => f.endsWith('_light.svg'))
  .map((f) => f.slice(0, -'_light.svg'.length))
  .sort()

const ts = `/**
 * GENERATED — do not hand-edit. Transcribed from material-extensions/vscode-material-icon-theme
 * at ${upstreamRev}, MIT (see \`ui/public/icons/LICENSE\`).
 *
 * The associations are upstream's, not ours. They are lifted out of \`src/core/icons/fileIcons.ts\`
 * and \`folderIcons.ts\` with upstream's own resolution applied first, so a key here answers
 * exactly what VS Code's Material Icon Theme answers:
 *
 *   * \`parseByPattern\` expanded (\`vite.config.ts\` exists because the \`vite\` entry carries
 *     \`patterns: { vite: Ecmascript }\`, not because anyone typed it here),
 *   * later entries win, because \`mapSpecificFileIcons\` assigns into the manifest in source
 *     order,
 *   * \`enabledFor\` icons outside the default \`'${ACTIVE_PACK}'\` pack dropped (so \`store/\` is a
 *     plain folder here, not \`folder-ngrx-store\`).
 *
 * The one place we do NOT copy upstream's table shape is folder names — see {@link BY_FOLDER}.
 *
 * Only the icons this project vendors are kept; every other association upstream knows resolves
 * to the generic fallback instead. Regenerate rather than edit: the source of truth is the
 * upstream repository, and a hand-added row is an association nobody can trace.
 */

/** Extension → icon, longest-suffix first. Keys are lowercase and carry no leading dot. */
export const BY_EXTENSION: Readonly<Record<string, string>> = {
${lit(byExt)}
}

/** Whole lowercased file name → icon. Beats {@link BY_EXTENSION}. */
export const BY_FILENAME: Readonly<Record<string, string>> = {
${lit(byName)}
}

/**
 * Canonical lowercased directory name → icon *stem*; \`-open\` is appended for an expanded row.
 *
 * Canonical means undecorated. Upstream's \`extendFolderNames\` writes \`x\`, \`.x\`, \`_x\`, \`-x\`
 * and \`__x__\` into its manifest because a VS Code icon theme is a flat table with nowhere to
 * put a rule; \`iconFor\` strips the decoration instead, so \`.github\` and \`.cargo\` resolve here
 * without an entry of their own.
 */
export const BY_FOLDER: Readonly<Record<string, string>> = {
${lit(byFolder)}
}

/**
 * Icon stems that have a \`<stem>_light.svg\` beside them — a second file drawn darker so the
 * mark does not disappear on a white background. Only these have one, and asking for \`_light\`
 * on anything else is a 404, which in an \`<img>\` is a blank row: hence the list rather than a
 * guess. Sorted, so a binary search or a \`Set\` is equally valid; \`iconFor\` uses a \`Set\`.
 *
 * Membership is decided by measurement, not taste — see the light pass in the generator.
 */
export const HAS_LIGHT_VARIANT: readonly string[] = [
${lightNames.map((n) => `  '${n}',`).join('\n')}
]
`

mkdirSync(join(UI, 'src/icons'), { recursive: true })
writeFileSync(OUT_MAP, ts)

// ---------------------------------------------------------------------------------------

const files = readdirSync(OUT_ICONS)
console.log(`svg files written: ${files.filter((f) => f.endsWith('.svg')).length}`)
console.log(`  file icons:   ${FILE_ICONS.length + 1}`)
console.log(`  folder icons: ${FOLDER_ICONS.length} closed + open, + generic folder pair`)
console.log(`  light variants: ${lightNames.length} (${generatedLight.length} generated, ${lightNames.length - generatedLight.length} upstream as-drawn)`)
console.log(
  `  darkened from upstream's own _light: ${generatedLight.filter(([, , u]) => u).map(([n]) => n).join(' ') || '(none)'}`,
)
console.log(`extension keys: ${byExt.length}`)
console.log(`filename keys:  ${byName.length}`)
console.log(`folder keys:    ${byFolder.length}`)
if (unmatchedIcons.length) console.log(`!! vendored but unreachable (file): ${unmatchedIcons.join(' ')}`)
if (unmatchedFolders.length) console.log(`!! vendored but unreachable (folder): ${unmatchedFolders.join(' ')}`)
const bytes = files.reduce((a, f) => a + readFileSync(join(OUT_ICONS, f)).length, 0)
console.log(`total bytes: ${bytes} (${(bytes / 1024).toFixed(1)} KiB)`)
console.log(`upstream: ${upstreamRev}`)
console.log('\nnow run: pnpm --dir ui run check:icons')

rmSync(MIT, { recursive: true, force: true })
