/**
 * Keeps the screenshot demo honest and contained. (M103)
 *
 * `src/demo/` is a second entry of the Vite project that boots the real app over a fake backend
 * so `scripts/demo-shots.mjs` can photograph it for the feature site. `tsc` already proves it
 * compiles against today's wire types. This proves the things a type cannot:
 *
 *  - **The scene list agrees everywhere.** `SCENE_IDS` (read as text by the capture script),
 *    the `SCENES` table, and every shot the site and the hero ask for. A scene the site names but
 *    the demo lacks is a broken image on the published page, and nothing else would notice.
 *  - **Every mocked command exists.** A handler for a command Rust no longer registers is a scene
 *    that renders from data the app can never receive again — a picture of a feature that is gone.
 *    Checked against `contract/commands.json`, the generated list of registered commands.
 *  - **The demo never reaches the app.** `index.html` must not load it and nothing outside
 *    `src/demo/` may import it: the fake backend answers every command, so a stray import would
 *    turn a real window into one that silently talks to nothing.
 *  - **`client.ts` stays the only `@tauri-apps/api` importer.** The fake installs itself by
 *    assignment onto `window.__TAURI_INTERNALS__`; importing the library's own mocks would make a
 *    second importer, which is the exception CLAUDE.md's wire-contract rule does not have.
 *
 * Every source is comment-stripped before it is searched: the house comments name the very
 * strings being checked for (this file included), and a needle in prose is not an import.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ui = join(dirname(fileURLToPath(import.meta.url)), '..')
const repo = join(ui, '..')
const demo = join(ui, 'src', 'demo')

let failures = 0
const fail = (msg) => {
  failures++
  console.error(`✗ ${msg}`)
}

const strip = (text) =>
  text
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:'"`])\/\/.*$/gm, '$1')
    .replace(/<!--[\s\S]*?-->/g, '')

const read = (path) => strip(readFileSync(path, 'utf8'))

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name)
    if (statSync(path).isDirectory()) walk(path, out)
    else if (/\.(ts|tsx)$/.test(name)) out.push(path)
  }
  return out
}

// --- the scene list ------------------------------------------------------------------------

const idsText = read(join(demo, 'sceneIds.ts'))
const idsBody = idsText.match(/SCENE_IDS\s*=\s*\[([\s\S]*?)\]/)
const ids = idsBody ? [...idsBody[1].matchAll(/'([a-z0-9-]+)'/g)].map((m) => m[1]) : []
if (ids.length === 0) fail('src/demo/sceneIds.ts: SCENE_IDS is empty or unreadable')
if (/\bimport\b/.test(idsText)) fail('src/demo/sceneIds.ts must stay import-free — the capture script reads it as text')

const table = read(join(demo, 'sceneTable.ts'))
const tableBody = table.match(/SCENES[^=]*=\s*\{([\s\S]*?)\n\}/)
const tableKeys = tableBody ? [...tableBody[1].matchAll(/^\s*'?([a-z0-9-]+)'?\s*:/gm)].map((m) => m[1]) : []
for (const id of ids) if (!tableKeys.includes(id)) fail(`scene '${id}' is in SCENE_IDS but not in SCENES`)
for (const key of tableKeys) if (!ids.includes(key)) fail(`scene '${key}' is in SCENES but not in SCENE_IDS`)

const content = read(join(repo, 'site', 'content.js'))
for (const [, shot] of content.matchAll(/shot:\s*'([a-z0-9-]+)'/g)) {
  if (!ids.includes(shot)) fail(`site/content.js shows '${shot}', which is not a demo scene`)
}
const index = readFileSync(join(repo, 'site', 'index.html'), 'utf8')
for (const [, shot] of strip(index).matchAll(/data-shot="([a-z0-9-]+)"/g)) {
  if (!ids.includes(shot)) fail(`site/index.html shows '${shot}', which is not a demo scene`)
}
const hero = readFileSync(join(repo, 'site', 'hero', 'hero.html'), 'utf8')
for (const [, shot] of strip(hero).matchAll(/data-src="([a-z0-9-]+)"/g)) {
  if (!ids.includes(shot)) fail(`site/hero/hero.html composes '${shot}', which is not a demo scene`)
}

// --- every mocked command exists -----------------------------------------------------------

const registered = new Set(JSON.parse(readFileSync(join(repo, 'contract', 'commands.json'), 'utf8')))
const demoFiles = walk(demo)
for (const file of demoFiles) {
  const text = read(file)
  const names = [
    ...[...text.matchAll(/handlers\.set\(\s*'([a-z0-9_:|]+)'/g)].map((m) => m[1]),
    // A `[name, handler]` entry of a `new Map(...)` table — the handler is what tells it apart
    // from any other string tuple (the file tree in `world.ts` is one).
    ...[...text.matchAll(/^\s*\[\s*'([a-z0-9_]+)'\s*,\s*(?:\(|async\b)/gm)].map((m) => m[1]),
  ]
  for (const name of names) {
    if (name.startsWith('plugin:')) continue
    if (!registered.has(name)) fail(`${relative(ui, file)} mocks '${name}', which Rust does not register`)
  }
}

// --- the demo never reaches the app --------------------------------------------------------

if (/demo/i.test(strip(readFileSync(join(ui, 'index.html'), 'utf8')))) fail('ui/index.html mentions the demo')
for (const file of walk(join(ui, 'src'))) {
  if (file.startsWith(demo)) continue
  if (/from\s+['"][^'"]*\/demo\//.test(read(file)) || /from\s+['"]@\/demo\//.test(read(file))) {
    fail(`${relative(ui, file)} imports from src/demo/`)
  }
}

// --- client.ts stays the only @tauri-apps importer -----------------------------------------

for (const file of demoFiles) {
  if (/from\s+['"]@tauri-apps\//.test(read(file))) fail(`${relative(ui, file)} imports @tauri-apps — only client.ts may`)
}

if (failures) {
  console.error(`check:demo — ${failures} problem(s)`)
  process.exit(1)
}
console.log(`check:demo — ${ids.length} scenes agree across the demo, the site and the hero; ${demoFiles.length} demo files contained`)
