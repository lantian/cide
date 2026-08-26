/**
 * Checks `src/icons/iconFor.ts` and the vendored icon set under `public/icons/`.
 *
 * Same shape as `check-tree-status.mjs` and for the same reason: there is no JS test runner
 * here, and the lookup is a pure function plus a generated table. Four things are pinned, in
 * rising order of how badly they fail:
 *
 *   1. **Known names map to the icon people expect.** The whole point of using the Material
 *      Icon Theme is recognition, so `.rs` must be the Rust cog and not something plausible.
 *      These assertions are the transcription's receipts.
 *   2. **Unknown falls back.** `dev.cide.ide.desktop` has no icon upstream and must land on
 *      the generic file rather than on nothing.
 *   3. **Directories differ from files, and open differs from closed.** A `src` file and a
 *      `src/` directory share a name and must not share an icon.
 *   4. **Every stem the table can produce exists on disk.** This is the one that ships: an
 *      `<img>` whose src 404s throws no error, logs nothing useful, and draws an empty box.
 *      A typo in a generated map, or an icon dropped from the vendor list while its
 *      associations stayed, is invisible in review and invisible in a screenshot of a tree
 *      that happens not to contain that file type.
 *
 * Run: `pnpm --dir ui run check:icons`
 */
import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const ICON_DIR = 'public/icons'

/**
 * `--panel` in each theme, read out of the stylesheet.
 *
 * `--panel` and not `--bg`: the file tree sits on the panel, and in the light theme that is
 * pure `#ffffff` while `--bg` is a hair off it — measuring against the softer of the two
 * would let an icon through that vanishes exactly where it is drawn. Light lives in the bare
 * `:root` block (see the note at the top of tokens.css), dark in `[data-theme='dark']`.
 */
function panelColours() {
  const css = readFileSync('src/styles/tokens.css', 'utf8')
  const at = (index) => {
    const m = /--panel:\s*(#[0-9a-fA-F]{3,8})\s*;/.exec(css.slice(index))
    if (!m?.[1]) throw new Error('tokens.css: could not find a --panel declaration')
    return m[1].toLowerCase()
  }
  const darkAt = css.indexOf("[data-theme='dark']")
  if (darkAt < 0) throw new Error("tokens.css: no [data-theme='dark'] block")
  return { light: at(0), dark: at(darkAt) }
}

const srgbToLinear = (v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)

function luminance(hex) {
  const full = hex.length === 4 ? `#${[...hex.slice(1)].map((c) => c + c).join('')}` : hex
  const [r, g, b] = [1, 3, 5].map((i) => srgbToLinear(parseInt(full.slice(i, i + 2), 16) / 255))
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/**
 * The best contrast any single colour in the icon manages against `bg`.
 *
 * *Best*, not worst: these are multi-colour marks and a pale highlight inside a dark glyph is
 * fine. What must never happen is that the whole icon is within a hair of the background,
 * which is what a low maximum means. Icons with no explicit colour at all (none today) are
 * skipped rather than assumed.
 */
function bestContrast(path, bg) {
  const src = readFileSync(path, 'utf8')
  const found = [...src.matchAll(/(?:fill|stroke|stop-color)="(#[0-9a-fA-F]{3}|#[0-9a-fA-F]{6})"/g)]
  if (found.length === 0) return Infinity
  return Math.max(...found.map((m) => contrast(m[1].toLowerCase(), bg)))
}

const out = mkdtempSync(join(tmpdir(), 'cide-icons-'))
try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/icons/iconFor.ts',
      '--outDir', out,
      // Pinned so the emitted layout is `<out>/icons/iconFor.js` regardless of how the
      // inferred common root falls out once `iconMap.ts` is pulled in beside it.
      '--rootDir', 'src',
      // CommonJS, unlike the sibling check scripts, because this module has a real *value*
      // import: `iconFor` reads the generated table. tsc emits `from './iconMap'` — correct
      // for a bundler and unloadable by Node's ESM resolver, which demands the extension.
      // CJS `require` resolves it, and `await import()` still hands back the named exports.
      '--module', 'commonjs',
      '--target', 'es2022',
      '--moduleResolution', 'node',
      '--strict',
      // The app's tsconfig sets this, and it is what makes every table lookup a
      // `string | undefined` the resolver has to handle rather than a `string` it may trust.
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const m = await import(`file://${join(out, 'icons', 'iconFor.js')}`)
  const map = await import(`file://${join(out, 'icons', 'iconMap.js')}`)

  let failed = 0
  const eq = (actual, expected, what) => {
    if (actual !== expected) {
      console.error(`FAIL ${what}\n  actual:   ${actual}\n  expected: ${expected}`)
      failed++
    }
  }

  const file = (name, theme = 'dark') => m.iconFor({ name, kind: 'file' }, theme)
  const dir = (name, expanded = false, theme = 'dark') =>
    m.iconFor({ name, kind: 'dir', expanded }, theme)

  // --- 1. known extensions are upstream's answer -------------------------------------------

  eq(file('main.rs'), 'rust', '.rs is the Rust icon')
  eq(file('treeStatus.ts'), 'typescript', '.ts is TypeScript')
  eq(file('FileTree.tsx'), 'react_ts', '.tsx is the React+TS icon, not plain TypeScript')
  eq(file('check-theme.mjs'), 'javascript', '.mjs is JavaScript')
  eq(file('generated.d.ts'), 'typescript-def', 'the longest extension wins over `ts`')
  eq(file('client.test.ts'), 'test-ts', 'and `test.ts` beats `ts` the same way')
  eq(file('tokens.css'), 'css', '.css')
  eq(file('App.module.css'), 'css', 'a CSS module is still CSS — `module.css` is not an entry')
  eq(file('theme-boot.js'), 'javascript', '.js')
  eq(file('commands.json'), 'json', '.json')
  eq(file('Cargo.toml'), 'toml', 'Cargo.toml has no name entry upstream and falls to `.toml`')
  eq(file('ci.yml'), 'yaml', '.yml')
  eq(file('README.md'), 'readme', 'README.md is the readme icon, not plain markdown')
  eq(file('BENCH.md'), 'markdown', 'but an ordinary .md is markdown')
  eq(file('logo.png'), 'image', '.png')
  eq(file('run.sh'), 'console', '.sh is the console icon')
  eq(file('user.proto'), 'proto', '.proto is the protobuf icon')
  eq(file('index.html'), 'html', '.html')

  // --- and known whole names beat any extension --------------------------------------------

  eq(file('package.json'), 'nodejs', 'package.json is Node, not JSON')
  eq(file('tsconfig.json'), 'tsconfig', 'tsconfig.json is its own icon')
  eq(file('pnpm-lock.yaml'), 'pnpm', 'pnpm-lock.yaml is pnpm, not YAML')
  eq(file('Cargo.lock'), 'lock', 'Cargo.lock resolves through the `.lock` extension')
  eq(file('vite.config.ts'), 'vite', 'vite.config.ts is Vite, not TypeScript')
  eq(file('.gitignore'), 'git', 'a dotfile with a name entry')
  eq(file('LICENSE'), 'license', 'an extensionless name, matched case-insensitively')
  eq(file('license'), 'license', 'and the same name in lower case')
  eq(file('tauri.conf.json'), 'tauri', 'tauri.conf.json is Tauri, not JSON')
  eq(file('Dockerfile'), 'docker', 'Dockerfile')

  // --- 2. the fallback ---------------------------------------------------------------------

  eq(
    file('dev.cide.ide.desktop'),
    m.DEFAULT_FILE_ICON,
    'an extension upstream has no icon for falls back to the generic file — and the walk down '
      + '`cide.ide.desktop` → `ide.desktop` → `desktop` must terminate, not throw',
  )
  eq(file('noextension'), m.DEFAULT_FILE_ICON, 'a bare name with no dot at all')
  eq(file(''), m.DEFAULT_FILE_ICON, 'the empty name, which a malformed row could produce')
  eq(file('.'), m.DEFAULT_FILE_ICON, 'a name that is only a separator')
  eq(dir('somethingnobodynamed'), m.DEFAULT_FOLDER_ICON, 'an unknown directory')
  eq(dir(''), m.DEFAULT_FOLDER_ICON, 'the empty directory name')

  // --- 3. directories are their own namespace ----------------------------------------------

  eq(dir('src'), 'folder-src', 'a known directory name')
  eq(dir('crates'), 'folder-lib', 'and one this repo actually has')
  eq(dir('src', true), 'folder-src-open', 'expanded appends `-open`')
  eq(dir('nothingknown', true), 'folder-open', 'including on the fallback folder')
  eq(
    file('src'),
    m.DEFAULT_FILE_ICON,
    'a FILE called `src` is not a folder icon — the two tables never see each other',
  )
  eq(
    dir('package.json'),
    m.DEFAULT_FOLDER_ICON,
    'and a directory called `package.json` is not the Node icon',
  )
  eq(dir('.github'), 'folder-github', 'a leading dot is stripped, as `extendFolderNames` does')
  eq(dir('.cargo'), 'folder-rust', 'so `.cargo` reaches the Rust folder via `cargo`')
  eq(dir('__tests__'), 'folder-test', 'and `__x__` unwraps')
  eq(dir('_test'), 'folder-test', 'as does a leading underscore')
  eq(dir('TARGET'), 'folder-target', 'matching is case-insensitive here too')

  // --- 3b. names that collide with Object.prototype ------------------------------------------
  //
  // The tables are object literals, so `BY_FILENAME['constructor']` is an inherited *function*
  // and `${it}` is `function Object() { [native code] }` — a stem with no file, drawn as a blank
  // row. Step 4 below cannot catch it: the bad stem is never a value in any table. Lower-casing
  // already saves `toString`; these are the members that survive it, and every one of them is a
  // legal name on a disk this app is pointed at.
  for (const evil of ['constructor', '__proto__', '__defineGetter__', '__lookupSetter__']) {
    eq(typeof file(evil), 'string', `a FILE named \`${evil}\` resolves to a string`)
    eq(file(evil), m.DEFAULT_FILE_ICON, `and \`${evil}\` falls back like any unknown name`)
    eq(typeof dir(evil), 'string', `a DIRECTORY named \`${evil}\` resolves to a string`)
    eq(dir(evil), m.DEFAULT_FOLDER_ICON, `and it falls back too`)
    eq(dir(evil, true), `${m.DEFAULT_FOLDER_ICON}-open`, `expanded, still the generic folder`)
    eq(file(`x.${evil}`), m.DEFAULT_FILE_ICON, `and as an EXTENSION, \`.${evil}\``)
  }

  // --- light variants ----------------------------------------------------------------------

  eq(
    file('app.js', 'light'),
    'javascript_light',
    'the light theme gets the darkened file: #ffca28 on white is 1.53:1, a smudge',
  )
  eq(file('main.rs', 'light'), 'rust_light', 'and `rust` has one too')
  eq(
    dir('src', true, 'light'),
    'folder-src-open_light',
    'a variant is keyed on the full stem, so the OPEN folder has its own',
  )
  eq(
    file('dev.cide.ide.desktop', 'light'),
    'file_light',
    'the fallback has a light variant as well — #90a4ae is 2.36:1 on white, and it is the '
      + 'most-drawn icon in any tree',
  )

  // --- 4. every stem the table can name exists on disk -------------------------------------

  if (!existsSync(ICON_DIR)) {
    console.error(`FAIL ${ICON_DIR} does not exist`)
    failed++
  } else {
    const onDisk = new Set(readdirSync(ICON_DIR).filter((f) => f.endsWith('.svg')))
    const LIGHT_SET = new Set(map.HAS_LIGHT_VARIANT)

    const stems = new Set([
      ...Object.values(map.BY_EXTENSION),
      ...Object.values(map.BY_FILENAME),
      m.DEFAULT_FILE_ICON,
    ])
    // Folder entries name a stem that is used two ways, so both have to be there.
    for (const s of [...Object.values(map.BY_FOLDER), m.DEFAULT_FOLDER_ICON]) {
      stems.add(s)
      stems.add(`${s}-open`)
    }

    let missing = 0
    for (const s of [...stems].sort()) {
      if (!onDisk.has(`${s}.svg`)) {
        console.error(`FAIL ${s}.svg is named by the map but not in ${ICON_DIR}/`)
        missing++
      }
    }
    // `HAS_LIGHT_VARIANT` is the promise `iconFor` acts on in the light theme; an entry with
    // no file is a row that is fine in dark and blank in light, which is the worst version of
    // this bug because half the reviewers never see it.
    for (const s of map.HAS_LIGHT_VARIANT) {
      if (!onDisk.has(`${s}_light.svg`)) {
        console.error(`FAIL ${s}_light.svg is promised by HAS_LIGHT_VARIANT but not on disk`)
        missing++
      }
      if (!stems.has(s)) {
        console.error(`FAIL ${s} has a light variant but is not a stem any lookup can return`)
        missing++
      }
    }
    failed += missing

    // The other direction: a vendored file no lookup can reach is dead weight, not a bug.
    // Reported, not failed — `-open` partners and light variants are reached indirectly.
    const reachable = new Set([...stems, ...map.HAS_LIGHT_VARIANT.map((s) => `${s}_light`)])
    const orphans = [...onDisk].map((f) => f.slice(0, -4)).filter((s) => !reachable.has(s))
    if (orphans.length > 0) console.warn(`note: ${orphans.length} unreachable icon(s): ${orphans.join(' ')}`)

    if (missing === 0) console.log(`icons: ${stems.size} stems, all present in ${ICON_DIR}/`)

    // --- 5. and every one of them is visible on the ground it is drawn on -------------------
    //
    // The ground measured is `--panel`, the panel's own background, and that is the *only*
    // claim this makes. A row also draws on `--chrome-hi` when hovered and `--sel` when
    // selected, both of which are darker than white: the darkening pass targets exactly 3.00:1
    // on #ffffff, so 78 of 126 stems sit at ~2.5:1 on a selected row and 86 at ~2.4:1 on a
    // hovered one. That is a deliberate trade — re-targeting the pass at #e6e6eb would darken
    // 45 more icons and cost the set the colour that is the point of using it — and not an
    // oversight. Change `MIN_CONTRAST_ON_WHITE` in `vendor-icons.mjs` if the trade changes.
    //
    // The set is pitched for a dark editor and this app's light theme is *white*, not VS
    // Code's #f3f3f3: `editorconfig` is four near-whites, 1.21:1 on #ffffff. That is not a
    // subtle regression, it is an invisible icon — and it is exactly what a reviewer working
    // in dark mode never sees. `vendor-icons.mjs` fixes it by generating a darkened `_light`
    // for anything under the bar; this is the assertion that the fix is still in place after
    // the next re-vendor, and that nobody has added an icon that skipped the pass.
    //
    // 3:1 is WCAG 1.4.11 for a non-text graphic. The grounds are read out of tokens.css
    // rather than pasted, so a palette change cannot leave this measuring the old white.
    const panels = panelColours()
    let dim = 0
    for (const s of [...stems].sort()) {
      const cases = [
        ['dark', `${s}.svg`, panels.dark],
        ['light', LIGHT_SET.has(s) ? `${s}_light.svg` : `${s}.svg`, panels.light],
      ]
      for (const [theme, f, bg] of cases) {
        if (!onDisk.has(f)) continue
        const best = bestContrast(join(ICON_DIR, f), bg)
        if (best < 3) {
          console.error(
            `FAIL ${f} is ${best.toFixed(2)}:1 on the ${theme} --panel (${bg}); needs 3:1. ` +
              'Re-run `node ui/scripts/vendor-icons.mjs`.',
          )
          dim++
        }
      }
    }
    failed += dim
    if (dim === 0) {
      console.log(
        `icons: every stem clears 3:1 on --panel in both themes ` +
          `(light ${panels.light}, dark ${panels.dark})`,
      )
    }
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('file icons: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
