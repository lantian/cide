/**
 * Checks the drawing pane: `src/panes/excalidrawKinds.ts` — the rules that decide which file
 * tab is a drawing, what its bytes hold, and what "dirty" means for a canvas — and the wiring
 * around them, each piece of which fails silently when it drifts. (M63)
 *
 * # Why these rules are worth a gate
 *
 * The component around them needs a window, a Tauri host, a real file and a canvas to render
 * at all, and there is no JS test runner in this project. So every rule that decides a file's
 * fate is in one import-free module the TypeScript in `node_modules` compiles on its own —
 * the arrangement `check-image.mjs` established — and the wiring is asserted on
 * comment-stripped source, because every comment in that wiring names the very words this
 * script looks for (CLAUDE.md's rule, learned three times).
 *
 * The silent failures, one assertion each:
 *
 * * `x.excalidraw.png` ends in `.png`, so `PaneBody` must ask the drawing rule **before** the
 *   image rule, or the file the pane exists to edit is shown as a picture.
 * * The engine is 2.7 MB and must reach the bundle through exactly one dynamic `import()`, or
 *   every window pays for it at startup — and nothing measures that but this.
 * * `EXCALIDRAW_ASSET_PATH` must be assigned before that import can run, and must spell the
 *   prefix the Vite plugin serves, or every label renders in a fallback face with nothing
 *   logged: the CDN fallback is blocked by the CSP, silently.
 * * Stamps compare by value — two answers are two objects — or every git-status recheck is a
 *   reload of a clean tab or a conflict bar on a dirty one.
 * * The key gate's `editorFocused` must exclude a drawing, or Ctrl+G and Ctrl+= are swallowed
 *   by editor commands that find no caret and log a line.
 *
 * Run: `pnpm --dir ui run check:excalidraw`
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-excalidraw-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (actual, what) => eq(Boolean(actual), true, what)

/** Comments out first, so a rule's own explanation cannot satisfy the rule. */
const strip = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')
const read = (path) => readFileSync(path, 'utf8')
const source = (path) => strip(read(path))

const walk = (dir, exts, acc = []) => {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) walk(full, exts, acc)
    else if (exts.some((e) => entry.endsWith(e))) acc.push(full)
  }
  return acc
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/excalidrawKinds.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const {
    DEFAULT_DRAWING_SUFFIX,
    drawingKindFor,
    sniffDrawing,
    mimeFor,
    PERSISTED_APP_STATE_KEYS,
    sceneFingerprint,
    sameFingerprint,
    sameStamp,
    formatDrawingBytes,
    drawingDetail,
    notADrawingMessage,
  } = await import(`file://${join(out, 'excalidrawKinds.js')}`)

  // --- the four suffixes the report named -----------------------------------------------
  eq(drawingKindFor('/p/flow.excalidraw'), 'json', '.excalidraw is JSON')
  /*
   * The suffix the file tree's *New ▸ Drawing* seeds its name box with. (M64)
   *
   * Pinned *through* `drawingKindFor` and not only by value, because the constant's whole job
   * is to be one of the four rows of `SUFFIXES`, and a constant that drifted off the table
   * would name a file the drawing pane never claims — an empty text buffer where a canvas was
   * promised, with nothing logged.
   */
  eq(DEFAULT_DRAWING_SUFFIX, '.excalidraw', 'a new drawing is plain JSON, not a picture')
  eq(
    drawingKindFor(`/p/flow${DEFAULT_DRAWING_SUFFIX}`),
    'json',
    'and a file named from the seed opens in the drawing pane',
  )
  eq(
    drawingKindFor(DEFAULT_DRAWING_SUFFIX),
    null,
    'while the seed ALONE is a dot-file and opens nowhere — which is the state the name box '
      + 'is in between the click and the first keystroke, and is why `checkName` refuses to '
      + 'commit a name that is still only the seed',
  )
  eq(drawingKindFor('/p/flow.excalidraw.json'), 'json', '.excalidraw.json is JSON')
  eq(drawingKindFor('/p/flow.excalidraw.svg'), 'svg', '.excalidraw.svg is SVG')
  eq(drawingKindFor('/p/flow.excalidraw.png'), 'png', '.excalidraw.png is PNG')
  eq(drawingKindFor('/p/FLOW.EXCALIDRAW.PNG'), 'png', 'an upper-case name is the same file')
  eq(drawingKindFor('C:\\Users\\u\\flow.Excalidraw'), 'json', 'backslashes separate too')
  eq(drawingKindFor('flow.excalidraw'), 'json', 'a bare basename still works')

  // --- and everything that must keep its current pane -----------------------------------
  for (const path of [
    '/p/logo.png',
    '/p/mark.svg',
    '/p/data.json',
    '/p/excalidraw.json',
    '/p/excalidraw',
    '/p/.excalidraw',
    '/p/.excalidraw.png',
    '/p/flow.excalidraw.',
    '/p/flow.excalidraw.png.bak',
    '/p/flow.excalidrawx',
    '/p/notes.excalidraw.md',
    '/home/u/dir.excalidraw/notes.txt',
    '/home/u/v1.excalidraw.png/logo',
  ]) {
    eq(drawingKindFor(path), null, `${path} is not routed to the drawing pane`)
  }

  // --- what the bytes hold ----------------------------------------------------------------
  const bytes = (...parts) =>
    new Uint8Array(parts.flatMap((p) => (typeof p === 'string' ? [...new TextEncoder().encode(p)] : p)))
  const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
  eq(sniffDrawing(bytes(PNG, 'rest')), 'png', 'the PNG signature')
  eq(sniffDrawing(bytes('<svg xmlns="http://www.w3.org/2000/svg">')), 'svg', 'an SVG')
  eq(sniffDrawing(bytes('<?xml version="1.0"?>\n<svg>')), 'svg', 'an SVG behind an XML declaration')
  eq(sniffDrawing(bytes('{"type":"excalidraw"}')), 'json', 'JSON')
  eq(sniffDrawing(bytes([0xef, 0xbb, 0xbf], '  \n{')), 'json', 'JSON behind a BOM and whitespace')
  eq(sniffDrawing(bytes('')), 'empty', 'a zero-byte file is a blank canvas')
  eq(sniffDrawing(bytes(' \n\t\r\n')), 'empty', 'a whitespace-only file is a blank canvas too')
  eq(sniffDrawing(bytes([0xef, 0xbb, 0xbf])), 'empty', 'and so is a lone BOM')
  eq(sniffDrawing(bytes([0x7f], 'ELF')), null, 'a binary that is not a PNG')
  eq(sniffDrawing(bytes('hello')), null, 'prose')
  eq(sniffDrawing(bytes([0x89, 0x50, 0x4e])), null, 'a PNG signature cut short is not a PNG')
  eq(mimeFor('png'), 'image/png', 'the type loadFromBlob sniffs for PNG')
  eq(mimeFor('svg'), 'image/svg+xml', 'the type loadFromBlob sniffs for SVG')
  eq(mimeFor('json'), 'application/json', 'the type loadFromBlob sniffs for JSON')

  // --- what a save persists, and therefore what may dirty the tab -------------------------
  eq(
    [...PERSISTED_APP_STATE_KEYS],
    ['viewBackgroundColor', 'gridSize', 'gridStep', 'gridModeEnabled'],
    'the appState keys marked export: true in 0.18.1, and no others — scroll, zoom, selection '
      + 'and tool defaults are not written and must not dirty the tab',
  )
  const state = { viewBackgroundColor: '#ffffff', gridSize: 20, gridStep: 5, gridModeEnabled: false }
  const base = sceneFingerprint(7, ['b', 'a'], state)
  ok(sameFingerprint(base, sceneFingerprint(7, ['a', 'b'], state)), 'file id order does not matter')
  ok(sameFingerprint(base, sceneFingerprint(7, ['b', 'a'], { ...state })), 'a copy of the state is the same state')
  ok(!sameFingerprint(base, sceneFingerprint(8, ['b', 'a'], state)), 'an element version moved')
  ok(!sameFingerprint(base, sceneFingerprint(7, ['b', 'a', 'c'], state)), 'a file was added')
  ok(!sameFingerprint(base, sceneFingerprint(7, ['a'], state)), 'a file was removed')
  ok(
    !sameFingerprint(base, sceneFingerprint(7, ['b', 'a'], { ...state, viewBackgroundColor: '#000' })),
    'the background changed',
  )
  ok(
    !sameFingerprint(base, sceneFingerprint(7, ['b', 'a'], { ...state, gridModeEnabled: true })),
    'the grid was switched on',
  )
  ok(sameFingerprint(null, null) && !sameFingerprint(base, null), 'null is only ever equal to null')

  // --- stamps by value ---------------------------------------------------------------------
  const stamp = { mtimeNanos: '1700000000000000001', len: 42 }
  ok(sameStamp(stamp, { ...stamp }), 'two stamps from two answers are the same stamp')
  ok(!sameStamp(stamp, { ...stamp, len: 43 }), 'a length moved')
  ok(!sameStamp(stamp, { ...stamp, mtimeNanos: '1700000000000000002' }), 'an mtime moved')
  ok(sameStamp(null, null) && !sameStamp(stamp, null) && !sameStamp(null, stamp), 'null is only null')

  // --- the status bar line -----------------------------------------------------------------
  eq(formatDrawingBytes(812), '812 B', 'bytes')
  eq(formatDrawingBytes(250 * 1024), '250 KiB', 'kibibytes — the MiB the Rust refusals print')
  eq(formatDrawingBytes(3.4 * 1024 * 1024), '3.4 MiB', 'mebibytes carry one decimal')
  eq(formatDrawingBytes(-1), '—', 'a nonsense size is not printed as a number')
  eq(drawingDetail('json', 42, 12 * 1024), 'Excalidraw · 42 elements · 12 KiB', 'the JSON line')
  eq(drawingDetail('png', 1, 812), 'Excalidraw · PNG · 1 element · 812 B', 'the PNG line, singular')
  eq(drawingDetail('svg', 0, 0), 'Excalidraw · SVG · 0 elements · 0 B', 'the SVG line, empty')
  ok(drawingDetail('json', 2, 2).includes('\u00b7'), 'the separator is U+00B7, as the editor readout')

  // --- the failure sentence -----------------------------------------------------------------
  const why = notADrawingMessage('shot.excalidraw.png', 'Couldn\'t load invalid file.')
  ok(why.startsWith('shot.excalidraw.png does not hold an Excalidraw drawing: '), `names the file: ${why}`)
  ok(why.endsWith('invalid file.'), `one full stop, not two: ${why}`)
  ok(!why.toLowerCase().includes('disk'), 'and does not blame the disk for a renamed picture')
  eq(
    notADrawingMessage('x.excalidraw', '   '),
    'x.excalidraw does not hold an Excalidraw drawing.',
    'an empty reason is left out rather than printed as a colon and nothing',
  )

  // --- the fork, and its order --------------------------------------------------------------
  const paneBody = source('src/panes/PaneBody.tsx')
  const drawingAt = paneBody.indexOf('drawingKindFor(editor.path)')
  const imageAt = paneBody.indexOf('imageKindFor(editor.path)')
  ok(drawingAt > 0 && imageAt > 0, 'PaneBody asks both rules')
  ok(
    drawingAt < imageAt,
    'and the drawing rule comes FIRST: `x.excalidraw.png` ends in `.png`, and the image fork '
      + 'would show the file the drawing pane exists to edit as a picture',
  )
  const drawingBranch = paneBody.slice(drawingAt, imageAt)
  ok(/<ExcalidrawPane\b/.test(drawingBranch), 'the branch renders the drawing pane')
  for (const prop of ['onScreen={onScreen}', 'tab={editor.tab}', 'project={project}', 'root={cwd}']) {
    ok(drawingBranch.includes(prop), `and passes ${prop} — a prop that stops one component short`)
  }
  ok(
    /^import \{ ExcalidrawPane \} from '\.\/ExcalidrawPane'/m.test(paneBody),
    'the pane is a STATIC import: a lazy pane renders nothing for a frame and is measured at zero',
  )

  // --- exactly one runtime importer of the engine --------------------------------------------
  const runtimeImporters = []
  const cssImporters = []
  for (const file of walk('src', ['.ts', '.tsx'])) {
    const src = source(file)
    if (/^import\s+'@excalidraw\/excalidraw\/index\.css'/m.test(src)) cssImporters.push(file)
    const re = /from\s+'@excalidraw\/excalidraw[^']*'/g
    for (const match of src.matchAll(re)) {
      const head = src.lastIndexOf('import', match.index)
      const statement = src.slice(head, match.index)
      if (!/^import\s+type\b/.test(statement)) {
        runtimeImporters.push(file)
        break
      }
    }
  }
  eq(
    runtimeImporters,
    ['src/panes/ExcalidrawSurface.tsx'],
    'the engine is imported at runtime by the surface module and by nothing else — anything '
      + 'else puts 2.7 MB in the entry bundle of every window',
  )
  eq(cssImporters, ['src/panes/ExcalidrawSurface.tsx'], 'and its stylesheet rides with it')

  const surface = source('src/panes/ExcalidrawSurface.tsx')
  ok(!/handleKeyboardGlobally=\{true\}/.test(surface) && /handleKeyboardGlobally=\{false\}/.test(surface),
    'handleKeyboardGlobally stays false: every tab is mounted and two drawings would both act on one key')
  ok(!/autoFocus=\{true\}/.test(surface) && /autoFocus=\{false\}/.test(surface),
    'autoFocus stays false: a restored workspace mounts every drawing at once')
  eq(
    (surface.match(/exportEmbedScene: true/g) ?? []).length,
    2,
    'both image exports embed the scene, or an .svg/.png written here reopens as a picture',
  )
  for (const hidden of ['loadScene: false', 'saveToActiveFile: false', 'export: false', 'saveAsImage: false', 'toggleTheme: false']) {
    ok(surface.includes(hidden), `${hidden} — a canvas action that ends in a Download a wry webview drops`)
  }
  ok(!/\.updateScene\(/.test(source('src/panes/ExcalidrawPane.tsx')) && !/\.updateScene\(/.test(surface),
    'a reload is a remount, never updateScene — the undo stack would resurrect the pre-reload scene')

  // --- the pane reaches the engine lazily, after the asset path is set --------------------------
  const pane = source('src/panes/ExcalidrawPane.tsx')
  ok(!/^import\s+(?!type\b)[^\n]*from\s+'\.\/ExcalidrawSurface'/m.test(pane),
    'the pane has no static import of the surface — only a type import, which is erased')
  const lazyAt = pane.indexOf("import('./ExcalidrawSurface')")
  ok(lazyAt > 0, 'the pane reaches the engine through a dynamic import()')
  const assetAt = pane.indexOf('EXCALIDRAW_ASSET_PATH =')
  ok(assetAt > 0 && assetAt < lazyAt,
    'EXCALIDRAW_ASSET_PATH is assigned before the first import() can run — the engine reads it per font')
  ok(/new URL\('excalidraw\/', document\.baseURI\)/.test(pane),
    'and it is `excalidraw/` relative to the document, the prefix the Vite plugin serves')
  for (const call of [
    'registerBuffer(', 'unregisterBuffer(', 'setDirty(', 'shouldAutosave(', 'autosaveDelay(',
    'fileChanged(', 'claimStatusReadout(', 'sameStamp(', 'sameFingerprint(', 'sniffDrawing(',
    'key={load.key}',
  ]) {
    ok(pane.includes(call), `the pane calls ${call}`)
  }
  // A call may wrap: `fileApi\n  .stat(` is how the formatter lays a chained call out.
  for (const method of ['readBytes', 'writeBytes', 'stat']) {
    ok(new RegExp(`fileApi\\s*\\.${method}\\(`).test(pane), `the pane calls fileApi.${method}(`)
  }
  ok(!/focusout/.test(pane), 'no focusout autosave: the engine\'s popovers live in document.body')
  ok(/drawingKindFor\(at\)/.test(pane) || /drawingKindFor\(pathRef\.current\)/.test(pane),
    'the save format is read from the path AT SAVE TIME, so a rename changes what the next save writes')

  // --- the fonts plugin, and the dependency ------------------------------------------------------
  const vite = source('vite.config.ts')
  ok(/plugins: \[react\(\), excalidrawFonts\(\)\]/.test(vite), 'the fonts plugin is installed')
  for (const piece of ['configureServer', 'closeBundle', 'dist/prod/fonts', "'/excalidraw/fonts'"]) {
    ok(vite.includes(piece), `the plugin has ${piece}`)
  }
  ok(/optimizeDeps: \{\s*include: \['@excalidraw\/excalidraw'\]/.test(vite),
    'the engine is pre-bundled up front, or its first open reloads the window mid-session')
  ok(!/public\/excalidraw/.test(vite), 'and nothing points at a committed copy under public/')
  const pkg = JSON.parse(read('package.json'))
  ok(/^\d/.test(pkg.dependencies['@excalidraw/excalidraw'] ?? ''), 'the engine is pinned exactly, no caret')
  const fonts = resolve('node_modules/@excalidraw/excalidraw/dist/prod/fonts')
  ok(existsSync(join(fonts, 'Excalifont')), `the package ships its fonts where the plugin reads them: ${fonts}`)

  // --- the wire mirror --------------------------------------------------------------------------
  const client = source('src/ipc/client.ts')
  ok(/getUint32\(0, true\)/.test(client) && /setUint32\(0, [^)]*, true\)/.test(client),
    'the frame prefix is read and written little-endian, as cide_ipc::frame writes it')
  for (const cmd of ["'file_read_bytes'", "'file_write_bytes'", "'file_stat'"]) {
    ok(client.includes(cmd), `client.ts names ${cmd}`)
  }

  // --- the key gate -------------------------------------------------------------------------------
  const context = source('src/keys/context.ts')
  ok(/'drawingFocused',/.test(context), 'drawingFocused is a derived flag')
  ok(/drawingKindFor\(focusedPath\)/.test(context) && /focusedTabPath\(boot\)/.test(context),
    'derived by name from the same function the handlers use')
  ok(/editorFocused: kind === 'editor' && !drawing,/.test(context) && /drawingFocused: drawing,/.test(context),
    'and editorFocused excludes it, or Ctrl+G and Ctrl+= are swallowed by editor commands')
  const commands = source('../crates/cide-core/src/commands.rs')
  ok(/"drawingFocused",/.test(commands.slice(commands.indexOf('pub const CONTEXT_FLAGS'))),
    'the flag is in the Rust vocabulary')
  ok(/"file\.save", "Save file", FILE\)\.when\("editorFocused \|\| drawingFocused"\)/.test(commands),
    'and Save file stays in the palette for a drawing')
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\n${failed} check(s) failed`)
  process.exit(1)
}
console.log('excalidrawKinds.ts: ok')
