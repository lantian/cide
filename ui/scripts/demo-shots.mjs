/**
 * Screenshots of the real UI, for the feature site, taken in headless Chromium. (M103)
 *
 *   pnpm --dir ui demo:shots                     every scene, both themes → site/img/
 *   pnpm --dir ui demo:shots --scene git,log     just those
 *   pnpm --dir ui demo:shots --theme dark        one theme
 *   pnpm --dir ui demo:shots --strict            fail a shot that asked for an unmocked command
 *   pnpm --dir ui demo:shots --dev               against a dev server (fast iteration, no build)
 *   pnpm --dir ui demo:shots --hero              only render site/hero/hero.html → the hero PNGs
 *   pnpm --dir ui demo:shots --out DIR           write somewhere other than site/img/
 *   pnpm --dir ui demo:shots --no-bootstrap      reuse src/demo/bootstrap.gen.json (skip cargo)
 *
 * # Why headless Chromium, and why this is safe on the machine it was written on
 *
 * cide is a WebKitGTK window on KDE/Wayland, and every way of photographing that window goes
 * through the compositor — `grim` hung under KWin and the machine went down right after it, once.
 * This never touches the compositor: `demo.html` boots the same `main.tsx` over a fake backend
 * (`src/demo/fakeTauri.ts`), and `--headless=new` renders it off-screen. The pictures are of the
 * real components, in the real tokens, over canned data — which is also why they are honest to
 * put on a site and not a mock-up drawn to look like the app.
 *
 * # Why not puppeteer
 *
 * `ui/package.json` pins every version exactly and a browser-automation package is a large,
 * fast-moving dependency to pin for one script. What this needs from the DevTools protocol is
 * eight methods, and Node 22 ships a `WebSocket`. So it speaks CDP itself.
 *
 * # Why a build and not the dev server by default
 *
 * The dev server's dependency optimiser can decide mid-run that it has found a new dependency and
 * reload the page ("new dependencies optimized, reloading"), which is a screenshot of a half-booted
 * window with nothing to say so. And port 1420 belongs to whatever cide the developer is running.
 * `--dev` stays for iteration, on its own port.
 */
import { spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { setTimeout as sleep } from 'node:timers/promises'

const ui = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repo = resolve(ui, '..')
const cache = join(ui, 'node_modules', '.cache', 'cide-demo')

const argv = process.argv.slice(2)
const flag = (name) => argv.includes(`--${name}`)
const option = (name) => {
  const i = argv.indexOf(`--${name}`)
  return i >= 0 ? argv[i + 1] : undefined
}

/** The scene ids, read from the one import-free module that declares them. */
function sceneIds() {
  const text = readFileSync(join(ui, 'src', 'demo', 'sceneIds.ts'), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/\/\/.*$/gm, '')
  const body = text.match(/SCENE_IDS\s*=\s*\[([\s\S]*?)\]/)
  if (!body) throw new Error('SCENE_IDS not found in src/demo/sceneIds.ts')
  return [...body[1].matchAll(/'([a-z0-9-]+)'/g)].map((m) => m[1])
}

const VIEWPORT = { width: 1440, height: 900 }
const DPR = Number(option('dpr') ?? 2)
const out = resolve(option('out') ?? join(repo, 'site', 'img'))
const themes = option('theme') ? [option('theme')] : ['dark', 'light']
const wanted = option('scene')?.split(',')
const scenes = wanted ?? sceneIds()

function chromium() {
  for (const bin of [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome']) {
    if (bin && existsSync(bin)) return bin
  }
  throw new Error('no Chromium found — set CHROMIUM=/path/to/chromium')
}

/** The real Bootstrap for the demo workspace, from the Rust core. See `cide-headless demo-bootstrap`. */
function writeBootstrap() {
  const run = spawnSync('cargo', ['run', '--locked', '-q', '-p', 'cide-headless', '--', 'demo-bootstrap'], {
    cwd: repo,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  })
  if (run.status !== 0) throw new Error('`cide-headless demo-bootstrap` failed')
  writeFileSync(join(ui, 'src', 'demo', 'bootstrap.gen.json'), run.stdout)
}

/** Serve the demo. Returns its base URL and a stop function. */
async function serve() {
  const vite = await import('vite')
  if (flag('dev')) {
    const server = await vite.createServer({ root: ui, server: { port: 1431, strictPort: false } })
    await server.listen()
    const url = server.resolvedUrls?.local[0] ?? 'http://127.0.0.1:1431/'
    return { base: url, stop: () => server.close() }
  }
  const dist = join(cache, 'dist')
  await vite.build({
    root: ui,
    logLevel: 'warn',
    build: {
      outDir: dist,
      emptyOutDir: true,
      sourcemap: false,
      rollupOptions: { input: join(ui, 'demo.html') },
    },
  })
  const server = await vite.preview({ root: ui, build: { outDir: dist }, preview: { port: 1432, strictPort: false } })
  const url = server.resolvedUrls?.local[0] ?? 'http://127.0.0.1:1432/'
  return { base: url, stop: () => new Promise((r) => server.httpServer.close(r)) }
}

/** Launch headless Chromium and connect to its first page over CDP. */
async function browser() {
  // Per process, so two captures can run at once (a scene author iterating beside a full run).
  const profile = join(cache, `profile-${process.pid}`)
  rmSync(profile, { recursive: true, force: true })
  mkdirSync(profile, { recursive: true })
  const child = spawn(chromium(), [
    '--headless=new',
    '--remote-debugging-port=0',
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--hide-scrollbars',
    '--font-render-hinting=none',
    `--window-size=${VIEWPORT.width},${VIEWPORT.height}`,
    'about:blank',
  ], { stdio: ['ignore', 'ignore', 'pipe'] })
  const port = await new Promise((resolveP, reject) => {
    let err = ''
    child.stderr.on('data', (d) => {
      err += d
      const m = err.match(/DevTools listening on ws:\/\/[^:]+:(\d+)\//)
      if (m) resolveP(Number(m[1]))
    })
    child.on('exit', (code) => reject(new Error(`chromium exited ${code}\n${err}`)))
  })
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()
  const page = targets.find((t) => t.type === 'page')
  const ws = new WebSocket(page.webSocketDebuggerUrl)
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j })
  let id = 0
  const pending = new Map()
  const handlers = new Map()
  ws.onmessage = (msg) => {
    const data = JSON.parse(msg.data)
    if (data.id && pending.has(data.id)) {
      const { resolve: ok, reject } = pending.get(data.id)
      pending.delete(data.id)
      if (data.error) reject(new Error(`${data.error.message}`))
      else ok(data.result)
    } else if (data.method) {
      for (const h of handlers.get(data.method) ?? []) h(data.params)
    }
  }
  const send = (method, params = {}) => new Promise((ok, reject) => {
    const n = ++id
    pending.set(n, { resolve: ok, reject })
    ws.send(JSON.stringify({ id: n, method, params }))
  })
  const on = (method, h) => handlers.set(method, [...(handlers.get(method) ?? []), h])
  const close = async () => {
    ws.close()
    const gone = new Promise((r) => child.once('exit', r))
    child.kill()
    await gone
    // Chromium's helpers can still be flushing into the profile a beat after the browser exits.
    try {
      rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
    } catch {
      // A stale profile in the cache directory costs disk, not correctness: each run uses its own.
    }
  }
  return { send, on, close }
}

const evaluate = async (cdp, expression) =>
  (await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result.value

/** `"ctrl+shift+p"` → two CDP key events. Only what the scenes use. */
async function press(cdp, chord) {
  const parts = chord.toLowerCase().split('+')
  const key = parts.pop()
  const mods = (parts.includes('alt') ? 1 : 0) | (parts.includes('ctrl') ? 2 : 0) | (parts.includes('shift') ? 8 : 0)
  const named = { space: [' ', 'Space', 32], enter: ['Enter', 'Enter', 13], escape: ['Escape', 'Escape', 27], tab: ['Tab', 'Tab', 9] }
  const [k, code, vk] = named[key] ?? [key, `Key${key.toUpperCase()}`, key.toUpperCase().charCodeAt(0)]
  const base = { modifiers: mods, key: mods & 8 && k.length === 1 ? k.toUpperCase() : k, code, windowsVirtualKeyCode: vk }
  await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base })
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base })
}

async function shoot(cdp, url, file, { strict }) {
  const errors = []
  cdp.errors = errors
  await cdp.send('Page.navigate', { url })
  const deadline = Date.now() + 30_000
  while (!(await evaluate(cdp, '!!document.documentElement.dataset.demoReady'))) {
    if (Date.now() > deadline) throw new Error(`${url}: never became ready`)
    await sleep(100)
  }
  const keys = (await evaluate(cdp, 'window.__demoKeys ?? []')) ?? []
  for (const chord of keys) {
    await press(cdp, chord)
    await sleep(250)
  }
  if (keys.length) await sleep(600)
  const boundary = await evaluate(cdp, '!!document.querySelector("[data-audit=panelFailed]")')
  const unmocked = (await evaluate(cdp, 'window.__demoUnmocked ?? []')) ?? []
  const { data } = await cdp.send('Page.captureScreenshot', {
    format: 'webp',
    quality: 82,
    captureBeyondViewport: false,
  })
  writeFileSync(file, Buffer.from(data, 'base64'))
  const problems = []
  if (errors.length) problems.push(`console errors:\n    ${errors.join('\n    ')}`)
  if (boundary) problems.push('a panel fell back to its error boundary')
  if (strict && unmocked.length) problems.push(`unmocked: ${unmocked.join(', ')}`)
  return { problems, unmocked }
}

async function main() {
  mkdirSync(out, { recursive: true })
  const cdp = await browser()
  await cdp.send('Page.enable')
  await cdp.send('Runtime.enable')
  cdp.on('Runtime.consoleAPICalled', (p) => {
    if (p.type === 'error') cdp.errors?.push(p.args.map((a) => a.value ?? a.description).join(' '))
  })
  cdp.on('Runtime.exceptionThrown', (p) => cdp.errors?.push(p.exceptionDetails.exception?.description ?? p.exceptionDetails.text))
  await cdp.send('Emulation.setDeviceMetricsOverride', { ...VIEWPORT, deviceScaleFactor: DPR, mobile: false })

  let failed = 0
  try {
    if (flag('hero')) {
      await hero(cdp)
      return
    }
    if (!flag('no-bootstrap') || !existsSync(join(ui, 'src', 'demo', 'bootstrap.gen.json'))) writeBootstrap()
    const server = await serve()
    try {
      for (const scene of scenes) {
        for (const theme of themes) {
          const file = join(out, `${scene}-${theme}.webp`)
          const url = `${server.base}demo.html?scene=${scene}&theme=${theme}`
          const { problems, unmocked } = await shoot(cdp, url, file, { strict: flag('strict') })
          const note = unmocked.length && !flag('strict') ? `  (unmocked: ${unmocked.join(', ')})` : ''
          if (problems.length) {
            failed++
            console.error(`✗ ${scene}-${theme}\n  ${problems.join('\n  ')}`)
          } else {
            console.log(`✓ ${scene}-${theme}${note}`)
          }
        }
      }
    } finally {
      await server.stop()
    }
    // The hero composites the shots just taken, so it is rendered again after them.
    if (!wanted) await hero(cdp)
  } finally {
    await cdp.close()
  }
  if (failed) {
    console.error(`${failed} shot(s) had problems`)
    process.exitCode = 1
  }
}

/**
 * `site/hero/hero.html` → `docs/img/hero.png` (the README's one image) and `site/img/og.png`
 * (the social card). A static page composed from the scene shots, so it is only as current as the
 * last full run.
 */
async function hero(cdp) {
  const page = join(repo, 'site', 'hero', 'hero.html')
  if (!existsSync(page)) return
  for (const [file, width, height] of [
    [join(repo, 'docs', 'img', 'hero.png'), 1600, 820],
    [join(repo, 'site', 'img', 'og.png'), 1200, 630],
  ]) {
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: file.endsWith('og.png') ? 1 : 2, mobile: false })
    await cdp.send('Page.navigate', { url: `${pathToFileURL(page)}?w=${width}&h=${height}` })
    const deadline = Date.now() + 15_000
    while (!(await evaluate(cdp, '!!document.documentElement.dataset.ready'))) {
      if (Date.now() > deadline) throw new Error('hero.html never became ready')
      await sleep(100)
    }
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' })
    writeFileSync(file, Buffer.from(data, 'base64'))
    console.log(`✓ ${file.slice(repo.length + 1)}`)
  }
  await cdp.send('Emulation.setDeviceMetricsOverride', { ...VIEWPORT, deviceScaleFactor: DPR, mobile: false })
}

await main()
