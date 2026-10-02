/** Wrapped terminal filenames, disk fallback, and the actual terminal Ctrl-click handler. */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync, mkdirSync, statSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import ts from 'typescript'
import xterm from '@xterm/xterm'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-path-buffer-'))
const previousHTMLElement = globalThis.HTMLElement
let failed = 0
const eq = (actual, expected, what) => {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    console.error(`FAIL ${what}\n  actual: ${JSON.stringify(actual)}\n  expected: ${JSON.stringify(expected)}`)
    failed++
  }
}
const ok = (value, what) => eq(Boolean(value), true, what)

/** Cell fixtures include wide and combining characters, plus unused cells at a row's edge. */
function buffer(rows, cols = 160) {
  const segmenter = new Intl.Segmenter('en', { granularity: 'grapheme' })
  const lines = rows.map((row) => {
    const text = typeof row === 'string' ? row : row.text
    const cells = []
    for (const { segment } of segmenter.segment(text)) {
      const width = /[界😀]/u.test(segment) ? 2 : 1
      cells.push({ chars: segment, width })
      if (width === 2) cells.push({ chars: '', width: 0 })
    }
    if (cells.length > cols) throw new Error('fixture row exceeds terminal columns')
    while (cells.length < cols) cells.push({ chars: '', width: 1 })
    return {
      length: cols,
      isWrapped: typeof row === 'string' ? false : Boolean(row.isWrapped),
      translateToString: (trim) => trim ? text.trimEnd() : text.padEnd(cols),
      getCell: (x, cell) => { cell.value = cells[x]; return cell },
    }
  })
  return {
    getLine: (y) => lines[y],
    getNullCell: () => ({ value: { chars: '', width: 1 },
      getChars() { return this.value.chars }, getWidth() { return this.value.width } }),
    cols,
    viewportY: 0,
  }
}

try {
  execFileSync('node', [
    'node_modules/typescript/bin/tsc',
    'src/terminal/pathBuffer.ts', 'src/terminal/pathExistence.ts',
    'src/terminal/clickGate.ts', 'src/terminal/taskLinks.ts',
    '--outDir', out, '--module', 'esnext', '--target', 'es2022',
    '--moduleResolution', 'bundler', '--strict', '--exactOptionalPropertyTypes',
    '--noUncheckedIndexedAccess',
  ], { cwd: UI, stdio: 'inherit' })
  const { readPathLine, pathRange, bufferPaths, resolveBufferPaths, MAX_PATH_ROWS } =
    await import(`file://${join(out, 'pathBuffer.js')}`)
  const { PathExistence, DISK_TTL_MS } = await import(`file://${join(out, 'pathExistence.js')}`)
  const { candidatePaths, outsidePaths } = await import(`file://${join(out, 'pathMatch.js')}`)
  const { linkAtCell } = await import(`file://${join(out, 'clickGate.js')}`)
  const ROOT = '/project'
  const bases = { cwds: [ROOT], roots: [ROOT] }
  const decoded = '.cide/worktrees/player-hover-outline/reports/e2e/unit_inspection_session/contact.png'
  const target = `${ROOT}/${decoded}`
  const rows = [
    '- Проверенные скриншоты (.cide/worktrees/player-',
    '  hover-outline/reports/e2e/unit\\',
    '  _inspection\\_session/contact.png).',
  ]
  const resolvedAt = (terminal, y, files, dirs = new Set(), context = bases) => {
    const current = readPathLine(terminal, y)
    if (!current) return []
    return resolveBufferPaths(bufferPaths(terminal, current), context,
      (path) => files.has(path), (path) => dirs.has(path))
  }

  for (let y = 0; y < rows.length; y++) {
    const terminal = buffer(rows)
    const found = resolvedAt(terminal, y, new Set([target]),
      new Set([`${ROOT}/.cide/worktrees/player-`]))
      .filter((path) => path.resolution.kind !== 'none')
    eq(found.length, 1, `row ${y}: the full screenshot outranks its partial directory`)
    eq(found[0]?.resolution, { kind: 'one', path: target }, `row ${y}: complete filename`)
    eq(found[0]?.candidate.text, decoded, `row ${y}: escape spanning two rows is decoded`)
    eq(found[0]?.ranges.length, 3, `row ${y}: separate clickable fragments`)
    for (const range of found[0]?.ranges ?? []) {
      ok(linkAtCell(range, range.start) && linkAtCell(range, range.end), 'both ends of each fragment click')
      ok(!linkAtCell(range, { x: range.start.x - 1, y: range.start.y }), 'indentation stays outside the link')
    }
  }

  // Native wrapping joins rows even when either row contains spaces or starts with a space.
  const native = buffer([
    { text: 'read "reports/my', isWrapped: false },
    { text: ' screenshot.png"', isWrapped: true },
  ], 16)
  eq(readPathLine(native, 1)?.text, 'read "reports/my screenshot.png"', 'native wrapped quoted filename')
  eq(resolvedAt(native, 1, new Set(['/project/reports/my screenshot.png']))[0]?.resolution,
    { kind: 'one', path: '/project/reports/my screenshot.png' }, 'native wrapped spaces resolve')
  const geometry = readPathLine(buffer(['界😀 e\u0301 src/a.ts'], 32), 0)
  const offset = geometry.text.indexOf('src/a.ts')
  eq(pathRange(geometry, offset, offset + 8),
    { start: { x: 8, y: 1 }, end: { x: 15, y: 1 } }, 'wide, surrogate and combining characters map to cells')
  eq(pathRange(readPathLine(buffer(['src/a.ts'], 8), 0), 0, 8),
    { start: { x: 1, y: 1 }, end: { x: 8, y: 1 } }, 'a path ending at the terminal edge has a valid last cell')
  const earlyWide = buffer([{ text: '1234567' }, { text: '界src/a.ts', isWrapped: true }], 10)
  const wideLine = readPathLine(earlyWide, 1)
  eq(pathRange(wideLine, 8, 16)?.start, { x: 3, y: 2 }, 'early-wrapped wide character leaves no offset drift')

  const outside = buffer(['/tmp/unit\\', '  _inspection/contact.png'])
  eq(resolvedAt(outside, 1, new Set(['/tmp/unit_inspection/contact.png']))
    .find((path) => path.reconstructed)?.resolution,
    { kind: 'one', path: '/tmp/unit_inspection/contact.png' }, 'wrapped absolute outside-project filename')

  const competing = buffer(['src/long', '  name', '  .ts'])
  for (let y = 0; y < 3; y++) {
    const ambiguous = resolvedAt(competing, y, new Set(['/project/src/longname', '/project/src/longname.ts']))
      .filter((path) => path.resolution.kind !== 'none')
    eq(ambiguous.length, 1, 'overlapping reconstructions are one ambiguous action')
    eq(ambiguous[0]?.resolution.kind, 'many', 'competing full paths never choose the longest or first')
    eq(new Set(ambiguous[0]?.resolution.paths).size, 2, 'ambiguity includes both verified targets')
  }
  const multi = { cwds: ['/one'], roots: ['/one', '/two'] }
  eq(resolvedAt(buffer(['src/long', '  name.ts']), 1,
    new Set(['/one/src/longname.ts', '/two/src/longname.ts']), new Set(), multi)[0]?.resolution.kind,
    'many', 'multi-root ambiguity is retained')
  for (const blocked of [
    ['src/long', '', '  name.ts'],
    ['src/long', '- name.ts'],
    ['src/long', '  ...name.ts'],
    ['src/long', 'some prose name.ts'],
    ['src/first.ts', 'src/second.ts'],
    ['https://host/src/long', '  name.ts'],
  ]) {
    const last = blocked.length - 1
    eq(resolvedAt(buffer(blocked), last, new Set(['/project/src/longname.ts']))
      .filter((path) => path.resolution.kind !== 'none'), [], 'unrelated lines never open a guessed filename')
  }
  eq(readPathLine(buffer(Array.from({ length: MAX_PATH_ROWS + 1 }, (_, y) =>
    ({ text: 'a', isWrapped: y > 0 }))), 20), null, 'native row limit refuses partial reconstruction')
  eq(readPathLine(buffer(['a'.repeat(4097)], 4100), 0), null, 'character limit refuses oversized lines')
  const chain = buffer(['src/a', ...Array.from({ length: 70 }, () => '  a')])
  ok(bufferPaths(chain, readPathLine(chain, 40)).every((path) => path.ranges.length <= MAX_PATH_ROWS),
    'hard-wrap reconstruction is bounded')
  eq(resolvedAt(buffer(['src/screenshot.', '  png']), 1,
    new Set(['/project/src/screenshot.png'])).find((path) => path.reconstructed)?.resolution,
    { kind: 'one', path: '/project/src/screenshot.png' }, 'wrapping immediately before an extension')
  eq(resolvedAt(buffer(['src/hover', '  -outline/contact.png']), 1,
    new Set(['/project/src/hover-outline/contact.png'])).find((path) => path.reconstructed)?.resolution,
    { kind: 'one', path: '/project/src/hover-outline/contact.png' }, 'a filename hyphen is not a list bullet')
  const dense = buffer(Array.from({ length: 64 }, () => 'src/a.ts'))
  ok(bufferPaths(dense, readPathLine(dense, 32)).every((path) => !path.reconstructed),
    'dense unrelated path logs do not trigger hundreds of speculative disk probes')

  // Exercise the installed xterm itself: ANSI output, native wrapping, hard TUI breaks and
  // early-wrapped wide characters. The harness fixtures share the real terminal link path.
  const escaped = String.raw`.cide/worktrees/player-hover-outline/reports/e2e/unit\_inspection\_session/contact.png`
  for (const [name, output] of [
    ['codex', `- Проверенные скриншоты (${escaped}).`],
    ['claude', `\x1b[1m●\x1b[0m Read(${decoded})`],
    ['opencode', `  → ${decoded}`],
  ]) {
    for (const cols of [20, 40, 80]) {
      const terminal = new xterm.Terminal({ cols, rows: 24 })
      try {
        await new Promise((done) => terminal.write(output, done))
        const active = terminal.buffer.active
        const logical = readPathLine(active, 0)
        ok(logical && logical.bottom > 0, `${name}: fixture really wraps in xterm at ${cols} columns`)
        for (let y = 0; y <= (logical?.bottom ?? -1); y++) {
          const path = resolvedAt(active, y, new Set([target]))
            .find((path) => path.resolution.kind !== 'none')
          eq(path?.resolution, { kind: 'one', path: target }, `${name}: native xterm row ${y} resolves`)
          for (const range of path?.ranges ?? []) {
            ok(range.start.x >= 1 && range.start.x <= cols && range.end.x >= 1 && range.end.x <= cols,
              'real xterm cell ranges stay within the grid')
          }
        }
      } finally { terminal.dispose() }
    }
  }
  const tui = new xterm.Terminal({ cols: 160, rows: 12 })
  try {
    await new Promise((done) => tui.write(rows.join('\r\n'), done))
    for (let y = 0; y < rows.length; y++) {
      eq(tui.buffer.active.getLine(y).isWrapped, false, 'TUI breaks really lack xterm wrap flags')
      eq(resolvedAt(tui.buffer.active, y, new Set([target]))[0]?.resolution,
        { kind: 'one', path: target }, 'real xterm hard-wrapped screenshot resolves from every row')
    }
  } finally { tui.dispose() }
  const wideTerminal = new xterm.Terminal({ cols: 10, rows: 8 })
  try {
    await new Promise((done) => wideTerminal.write('123456789界src/a.ts', done))
    const line = readPathLine(wideTerminal.buffer.active, 1)
    eq(pathRange(line, line.text.indexOf('src/a.ts'), line.text.length)?.start,
      { x: 3, y: 2 }, 'real xterm wide character wraps early without shifting the path range')
  } finally { wideTerminal.dispose() }
  const spacedTerminal = new xterm.Terminal({ cols: 17, rows: 8 })
  try {
    await new Promise((done) => spacedTerminal.write('read "reports/my screenshot.png"', done))
    const line = readPathLine(spacedTerminal.buffer.active, 1)
    eq(line.text, 'read "reports/my screenshot.png"', 'a printed trailing space survives native wrapping')
    eq(resolvedAt(spacedTerminal.buffer.active, 1, new Set(['/project/reports/my screenshot.png']))[0]?.resolution,
      { kind: 'one', path: '/project/reports/my screenshot.png' }, 'wrapped filename with a space opens')
  } finally { spacedTerminal.dispose() }

  // Real files behind an empty index: ignored screenshots and generated output still resolve.
  const fixture = join(out, 'project')
  const image = join(fixture, decoded)
  mkdirSync(join(image, '..'), { recursive: true })
  writeFileSync(image, 'png fixture')
  writeFileSync(join(fixture, '.gitignore'), '.cide/worktrees/\n')
  const calls = []
  let now = 0
  const disk = new Map([[target, 'file'], ['/tmp/unit_inspection/contact.png', 'file']])
  const cache = new PathExistence(async (_project, paths) => {
    calls.push(['index', paths]); return paths.map(() => 'absent')
  }, async (paths) => {
    calls.push(['disk', paths]); return paths.map((path) => disk.get(path) ?? 'absent')
  }, () => now)
  await cache.lookup(ROOT, [target, target], [])
  eq(cache.kind(target), 'file', 'index miss falls back to disk')
  eq(calls.map(([kind]) => kind), ['index', 'disk'], 'deduplicated bounded lookup')
  await cache.lookup(ROOT, [target], [])
  eq(calls.length, 2, 'hover reuses fresh fallback answers')
  disk.delete(target)
  await cache.lookup(ROOT, [target], [], true)
  eq(cache.kind(target), 'absent', 'activation refreshes a deleted file')
  disk.set(target, 'file')
  now += DISK_TTL_MS
  await cache.lookup(ROOT, [target], [])
  eq(cache.kind(target), 'file', 'ignored files reappear after the TTL despite a cached index miss')
  const absent = '/project/reports/new.png'
  await cache.lookup(ROOT, [absent], [])
  disk.set(absent, 'file')
  await cache.lookup(ROOT, [absent], [], true)
  eq(cache.kind(absent), 'file', 'newly created file opens immediately on activation')
  cache.invalidate([absent])
  eq(cache.kind(absent), undefined, 'watcher invalidates both lookup sources')
  await cache.lookup(ROOT, [], ['/tmp/unit_inspection/contact.png'])
  eq(cache.kind('/tmp/unit_inspection/contact.png'), 'file', 'outside-project lookup remains supported')
  const fastCalls = []
  const fast = new PathExistence(async (_project, paths) => paths.map(() => 'file'),
    async (paths) => { fastCalls.push(paths); return paths.map(() => 'absent') })
  await fast.lookup(ROOT, ['/project/src/a.ts'], [])
  eq(fastCalls.length, 0, 'indexed hover stays free of filesystem probes')
  await fast.lookup(ROOT, ['/project/src/a.ts'], [], true)
  eq(fast.kind('/project/src/a.ts'), 'absent', 'fresh disk refusal overrides a stale positive index hit')
  let attempts = 0
  const retry = new PathExistence(async () => { throw Error('not indexed yet') }, async (paths) => {
    if (attempts++ === 0) throw Error('temporary IPC failure')
    return paths.map(() => 'file')
  })
  await retry.lookup(ROOT, [target], [])
  await retry.lookup(ROOT, [target], [])
  eq(retry.kind(target), 'file', 'failed probes are retried rather than cached as missing')
  const batchSizes = []
  const batches = new PathExistence(async (_project, paths) => paths.map(() => 'absent'), async (paths) => {
    batchSizes.push(paths.length); return paths.map(() => 'absent')
  })
  await batches.lookup(ROOT, Array.from({ length: 300 }, (_, n) => `/project/${n}/a.ts`), [])
  eq(batchSizes, [128, 128, 44], 'disk probes respect the backend batch cap')
  const actual = new PathExistence(async (_project, paths) => paths.map(() => 'absent'), async (paths) =>
    paths.map((path) => { try { return statSync(path).isFile() ? 'file' : 'dir' } catch { return 'absent' } }))
  await actual.lookup(fixture, [image], [])
  eq(actual.kind(image), 'file', 'an ignored screenshot on real disk is discoverable')

  // Transpile the actual adapter with only its Tauri services replaced. Full typechecking is
  // done by check:typecheck; this fixture exercises event capture, ranges and the open callback.
  writeFileSync(join(out, 'linkHarness.js'), `
    export const harness = { notices: [], opened: [], urls: [], files: new Map(), calls: [] };
    export const notify = (message) => harness.notices.push(message);
    export const isWebUrl = (url) => /^https?:/.test(url);
    export const openWebLink = (url) => harness.urls.push(url);
    export const events = { onFsChanged: async (callback) => { harness.changed = callback; } };
    export const fs = {
      pathsExist: async (_project, paths) => { harness.calls.push('index'); return paths.map(() => null); },
      statPaths: async (paths) => { harness.calls.push('disk'); return paths.map((path) => harness.files.get(path) ?? null); }
    };
    export const session = { cwd: async () => null };
  `)
  const adapter = readFileSync(join(UI, 'src/terminal/pathLinks.ts'), 'utf8')
  let compiled = ts.transpileModule(adapter, { compilerOptions: { target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext } }).outputText
  compiled = compiled.replace(/from '@\/[^']+'/g, "from './linkHarness.js'")
    .replace(/from '(\.\/[^']+)'/g, (full, path) => path.endsWith('.js') ? full : `from '${path}.js'`)
  writeFileSync(join(out, 'pathLinks.js'), compiled)
  const { attachPathLinks, setPathLinkEnv } = await import(`file://${join(out, 'pathLinks.js')}`)
  const { harness } = await import(`file://${join(out, 'linkHarness.js')}`)
  class Element {
    listeners = new Map()
    addEventListener(name, listener, capture) { this.listeners.set(name, { listener, capture }) }
    removeEventListener(name) { this.listeners.delete(name) }
    querySelector() { return this }
    getBoundingClientRect() { return { left: 0, top: 0, width: 1600, height: 100 } }
  }
  globalThis.HTMLElement = Element
  const drain = async () => { for (let n = 0; n < 5; n++) await new Promise(setImmediate) }
  for (const name of ['codex', 'claude', 'opencode']) {
    const harnessRoot = `/${name}`
    const harnessTarget = `${harnessRoot}/${decoded}`
    const el = new Element()
    const active = buffer(rows)
    let provider
    const term = { buffer: { active }, element: el, cols: 160, rows: 5,
      getSelection: () => '', registerLinkProvider: (value) => { provider = value; return { dispose() {} } } }
    setPathLinkEnv(name, { project: harnessRoot, roots: [harnessRoot], cwd: harnessRoot,
      open: (path, at) => harness.opened.push({ path, at }), reveal: () => {} })
    harness.files.set(harnessTarget, 'file')
    const dispose = attachPathLinks({ term }, el, { paneId: name, session: () => null })
    const links = await new Promise((done) => provider.provideLinks(2, done))
    eq(links?.length, 3, `${name}: all fragments offered by the real provider`)
    eq(el.listeners.get('mousedown').capture, true, `${name}: Ctrl-click is captured before the child`)
    for (const link of links ?? []) {
      let swallowed = false
      el.listeners.get('mousedown').listener({ button: 0, ctrlKey: true, metaKey: false,
        clientX: (link.range.start.x - 0.5) * 10, clientY: (link.range.start.y - 0.5) * 20,
        preventDefault() {}, stopPropagation() { swallowed = true } })
      ok(swallowed, `${name}: capture swallows the press synchronously`)
      await drain()
      eq(harness.opened.at(-1), { path: harnessTarget, at: null }, `${name}: every fragment opens the screenshot`)
    }
    eq(harness.notices, [], `${name}: successful clicks do not show a false missing-path notification`)
    harness.files.delete(harnessTarget)
    const count = harness.opened.length
    const link = links?.[2]
    if (link) link.activate({ ctrlKey: true })
    await drain()
    eq(harness.opened.length, count, `${name}: activation rechecks deleted files`)
    ok(harness.notices.length > 0, `${name}: a missing file still answers the swallowed click`)
    harness.notices.length = 0
    dispose()
    setPathLinkEnv(name, null)
  }
  // Relative reconstructions still cannot invent an outside-project target.
  eq(candidatePaths('../../tmp/shot.png', bases), [], 'relative paths stay contained')
  eq(outsidePaths('../../tmp/shot.png', bases), [], 'outside lookup requires a displayed absolute path')

  if (failed) throw Error(`${failed} terminal buffer check(s) failed`)
  console.log('terminal wrapped paths and disk fallback: ok')
} finally {
  if (previousHTMLElement === undefined) delete globalThis.HTMLElement
  else globalThis.HTMLElement = previousHTMLElement
  rmSync(out, { recursive: true, force: true })
}
