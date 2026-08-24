/**
 * Checks the editor's colour-scheme palette — the `--tk-*` family — and the module that applies
 * one. (M24)
 *
 * # What is actually at risk here
 *
 * The set of roles is spelled in **four files that cannot import one another**:
 *
 *   - `crates/cide-ipc/src/theme.rs`  — what Rust normalises an imported theme against
 *   - `src/editor/scheme.ts`          — what the DOM applier writes and clears
 *   - `src/editor/highlight.css`      — a plain stylesheet, because CodeMirror writes the
 *                                       literal class name and a module would hash it
 *   - `src/styles/tokens.css`         — the compiled-in `cide` scheme, in two palette blocks
 *
 * Four copies is the floor, not a choice. Every way they can disagree is silent on screen: a
 * role declared in `tokens.css` and read nowhere is a colour nobody sees, a role read in
 * `highlight.css` and declared nowhere paints *nothing at all* (an undefined custom property is
 * not a colour, so the span inherits), and a role Rust does not know about is a role an imported
 * theme can never fill. None of those fails a build or looks wrong at a glance.
 *
 * That is the same argument `check-ui-scale.mjs` makes for the four copies of the base font
 * size, and the same closed-in-both-directions rule `check-ui-icons.mjs` applies to the icon set.
 *
 * # What this does NOT cover
 *
 *   - That a scheme reaches the screen. There is no DOM here; `applyScheme` is exercised against
 *     a stub with the same three methods, which proves the decisions and nothing about paint.
 *   - The conversion from a VS Code theme. That is `cide_core::scheme` and is covered by
 *     `cargo test -p cide-core scheme`, where it belongs — it is a scope matcher with precedence
 *     rules and it needs a table of cases, not a stylesheet.
 *   - Anything about the chrome palette. `check-theme.mjs` owns that, and the two families are
 *     deliberately disjoint — an assertion below is exactly that they do not collide.
 *
 * Run: `pnpm --dir ui run check:scheme`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-scheme-'))
let checks = 0
let failed = 0

const eq = (actual, expected, what) => {
  checks++
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (actual, what) => eq(actual === true, true, what)

/**
 * The floor a token role must clear — against the buffer's background **and** against its
 * selection.
 *
 * # The selection is the half this file did not check, and it was hiding a real bug
 *
 * The first version measured ink against `--tk-bg` only, and the report that followed was *"when
 * I select the code in dark theme I don't see the text"*. It was true and it was worse than it
 * sounds: the dark selection sat 1.60:1 above the background, so it raised the floor under every
 * ink at once, and comments landed at **1.73:1** — invisible — with punctuation at 3.09:1.
 *
 * A selection is a *ground*. Checking ink against one ground and then painting it on another is
 * not a weaker check, it is a check of the wrong thing, and the palette had shipped that way
 * since M3 with a comment in `EditorSurface.module.css` asserting a 4.45:1 worst case that had
 * been measured against `--panel` and was never true of the pair.
 *
 * # 3.0, and the carve-out that used to be here is gone
 *
 * This was 2.5, argued as "a comment is deliberately quiet and `#5c5c66` is 2.76:1". That was
 * true and it was the wrong conclusion: 2.76:1 was already marginal, and it was the reason the
 * selection could not be fixed by darkening the selection alone. Both moved, the palette now
 * clears 3.0 on both grounds in both themes, and the exemption stopped being a decision.
 *
 * 3.0 is `check-theme.mjs`'s `TERMINAL_MIN_CONTRAST`, deliberately: it is the same question about
 * the same kind of thing, and two floors would be two numbers to argue about.
 */
const INK_FLOOR = 3

/**
 * The share of its unselected contrast an ink must keep when it is selected.
 *
 * **This is the assertion that actually prevents the bug, and the absolute floor above is the
 * backstop.** A floor asks "is this pair legible"; this asks "does selecting cost anything", and
 * only the second one scales — the palette can be retuned freely as long as the selection stays
 * out of the way, and no future colour can be quietly ruined by the ground it lands on.
 *
 * It exists because an absolute floor was tried first and was not enough. The dark selection was
 * darkened from the chrome's `#2b3a55` to `#132a4d`, ink cleared 3:1, this gate went green — and
 * the report came back, because 3:1 is the WCAG floor for *large* text and the buffer is 12.5px.
 * Chasing a higher absolute floor is unbounded: contrast against a selection is
 * `(ink + 0.05) / (selection + 0.05)`, so the selection's luminance taxes every ink at once, and
 * a palette whose quietest ink is 3.84:1 unselected cannot reach 4.5:1 on *any* selection —
 * against a pure black one it tops out at 4.42:1.
 *
 * The way out is a selection that carries no luminance, only chroma, which is what
 * `--tk-sel: #0a1450` is. Then the question stops being "how much does the selection cost" and
 * becomes "nothing", and this ratio is how that is stated.
 *
 * 0.80 rather than the 0.94 the dark palette achieves: the light theme's warm `#ffe7d9` sits at
 * 0.84 and is fine at it — every light ink clears 4.18:1 on it — because losing a sixth of a
 * large ratio is not the same as losing a sixth of a small one. The threshold is set where it
 * fails every state known to be bad (the shipped bug scored 0.63, the insufficient fix 0.79) and
 * passes both palettes as they stand.
 */
const SELECTION_KEEP = 0.8

/**
 * The floor for the gutter's line numbers, which are ink but are not a token role.
 *
 * Lower on purpose, and measured against the background only. A line number is furniture: it is
 * never drawn on a selection — `.cm-activeLineGutter` grounds the caret's row on `--tk-sel` but
 * colours the number `--dim`, not `--tk-gutter` — and brighter numbers compete with the code they
 * are numbering. Its own floor rather than an exemption inside the loop, because an exemption is
 * a hole and a named number is a decision.
 */
const GUTTER_FLOOR = 2.5

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/editor/scheme.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const {
    BUILTIN_SCHEME,
    SCHEME_SURFACE,
    SCHEME_TOKENS,
    SCHEME_ROLES,
    propertyFor,
    schemeToApply,
    applyScheme,
    schemeIsPainted,
    schemeChoices,
  } = await import(`file://${join(out, 'scheme.js')}`)

  // --- 1. the four copies agree ------------------------------------------------------------

  const rust = readFileSync('../crates/cide-ipc/src/theme.rs', 'utf8')
  const rustList = (name) => {
    const block = new RegExp(`${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(rust)
    if (block === null) return null
    return [...block[1].matchAll(/"([a-z]+)"/g)].map((m) => m[1])
  }
  eq(rustList('SCHEME_SURFACE'), [...SCHEME_SURFACE], 'Rust and TS agree on the surface roles')
  eq(rustList('SCHEME_TOKENS'), [...SCHEME_TOKENS], 'Rust and TS agree on the token roles')
  eq(
    /pub const BUILTIN_SCHEME: &str = "([a-z-]+)"/.exec(rust)?.[1],
    BUILTIN_SCHEME,
    'Rust and TS agree on the builtin scheme id',
  )
  eq(SCHEME_ROLES, [...SCHEME_SURFACE, ...SCHEME_TOKENS], 'the role list is surface then tokens')
  eq(
    new Set(SCHEME_ROLES).size,
    SCHEME_ROLES.length,
    'no role is listed twice — a duplicate would be a property written and then written again',
  )

  /*
   * `highlight.css` reads one `--tk-*` per token role, plus the two surface properties it sets
   * on the bare `.cm-editor`. The other four surface roles are read from `*.module.css` files,
   * which is what the next block covers.
   */
  const highlightCss = readFileSync('src/editor/highlight.css', 'utf8')
  const readIn = (css) => new Set([...css.matchAll(/var\((--tk-[a-z-]+)\)/g)].map((m) => m[1]))
  const tokenProps = new Set([...SCHEME_TOKENS].map(propertyFor))
  const highlightReads = readIn(highlightCss)
  for (const prop of tokenProps) {
    ok(highlightReads.has(prop), `highlight.css paints ${prop}`)
  }
  /*
   * The three surface roles `highlight.css` owns, and the third one is there for a reason worth
   * pinning. `--tk-sel` cannot live in a CSS module: `@codemirror/view`'s base theme claims the
   * selection at specificity (0,6,0) and a module's hashed `.body` prefix cannot reach that, so
   * the rule that used to sit in `EditorSurface.module.css` was silently inert and the editor
   * painted CodeMirror's light-mode lavender. Moving it back would restore that bug with nothing
   * failing, which is exactly what this list is for.
   */
  eq(
    [...highlightReads].filter((p) => !tokenProps.has(p)).sort(),
    ['--tk-bg', '--tk-fg', '--tk-sel'],
    'highlight.css reads exactly the token roles plus the ground, the ink and the selection',
  )
  ok(
    /html:root[^{]*\.cm-selectionBackground/.test(highlightCss),
    'the selection rule out-specifies `@codemirror/view`’s own (0,6,0) base-theme rule',
  )
  eq(
    [...highlightCss.matchAll(/\.(cide-tk-[a-z]+)\s*\{/g)].map((m) => m[1]).length,
    SCHEME_TOKENS.length,
    'one class rule per token role — the count `highlight.ts` builds its table from',
  )

  /*
   * The four remaining surface roles have to be read *somewhere*, or an imported scheme sets a
   * selection colour that nothing paints. Asserted against the whole of `src` rather than
   * against a named file, because which stylesheet owns the caret is not this gate's business —
   * that it is owned at all is.
   */
  const allCss = readFileSync('src/editor/EditorSurface.module.css', 'utf8')
    + readFileSync('src/panes/DiffPane.module.css', 'utf8')
    + readFileSync('src/editor/minimap.ts', 'utf8')
  const surfaceReads = new Set([...readIn(allCss), ...readIn(highlightCss)])
  for (const role of SCHEME_SURFACE) {
    ok(surfaceReads.has(propertyFor(role)), `something paints ${propertyFor(role)}`)
  }

  // --- 2. tokens.css declares the whole family, in both palettes ----------------------------

  const tokensCss = readFileSync('src/styles/tokens.css', 'utf8')
  const rules = leafRules(tokensCss)
  const declared = new Set(
    [...tokensCss.matchAll(/(^|[;{\s])(--tk-[a-z-]+)\s*:/gm)].map((m) => m[2]),
  )
  eq(
    [...declared].sort(),
    SCHEME_ROLES.map(propertyFor).sort(),
    'tokens.css declares exactly the role set — no extra, none missing',
  )

  /*
   * **Completeness, before the cascade is allowed to hide anything.**
   *
   * This is the assertion the first draft of this file did not have, and a deliberately broken
   * `tokens.css` walked straight past it: `resolve` below applies the base block and *then* the
   * theme's overrides, so a `--tk-doc` missing from `[data-theme='dark']` silently resolves to
   * the light value and every per-role check still passes. The symptom on screen is one role
   * wearing the other theme's colour — legible, plausible, and wrong.
   *
   * So the rule is `check-theme.mjs:206`'s, applied to this family: a document-level rule that
   * declares *part* of it must declare *all* of it. A block is either a palette or it is not.
   */
  const family = SCHEME_ROLES.map(propertyFor)
  const themeLevel = (selector) =>
    selector
      .split(',')
      .every((p) => /^(:root|html|\[data-theme=['"]?[\w-]+['"]?\])+$/.test(p.trim()))
  let paletteBlocks = 0
  for (const rule of rules) {
    const has = family.filter((prop) => new RegExp(`(^|[;{\\s])${prop}\\s*:`).test(rule.body))
    if (has.length === 0) continue
    ok(themeLevel(rule.selector), `\`${rule.selector}\` carries scheme tokens at document level`)
    paletteBlocks++
    eq(
      family.filter((prop) => !has.includes(prop)),
      [],
      `\`${rule.selector}\` carries part of the editor palette, so it must carry all of it`,
    )
  }
  eq(paletteBlocks, 2, 'exactly two blocks carry the editor palette — one per theme')

  const palettes = {}
  for (const theme of ['light', 'dark']) {
    const values = resolve(rules, theme, SCHEME_ROLES.map(propertyFor))
    palettes[theme] = values
    for (const role of SCHEME_ROLES) {
      ok(
        typeof values[propertyFor(role)] === 'string',
        `the ${theme} palette declares ${propertyFor(role)}`,
      )
    }
  }

  /*
   * The two palettes must actually differ. A block copied from the other and left unedited
   * passes every assertion above and paints one theme's buffer in the other's colours.
   */
  ok(
    SCHEME_TOKENS.some((r) => palettes.light[propertyFor(r)] !== palettes.dark[propertyFor(r)]),
    'the two palettes give the editor different colours',
  )

  /*
   * No `--tk-*` may collide with a chrome token. The whole point of the family is that the
   * editor's palette and the chrome's — which is also the terminal's ANSI palette — are
   * separate decisions; a shared *name* would be a shared decision by another route.
   */
  const chrome = new Set(
    [...tokensCss.matchAll(/(^|[;{\s])(--[a-z0-9-]+)\s*:/gm)]
      .map((m) => m[2])
      .filter((t) => !t.startsWith('--tk-')),
  )
  for (const role of SCHEME_ROLES) {
    ok(!chrome.has(propertyFor(role).slice(5)), `${propertyFor(role)} does not shadow a chrome token`)
  }

  // --- 3. the compiled-in scheme is legible against its own ground --------------------------

  for (const theme of ['light', 'dark']) {
    /*
     * **Both grounds.** A token is painted on the background most of the time and on the
     * selection whenever the user drags across it, and the second is the one that was never
     * checked — see `INK_FLOOR`. The active line needs no third pass: it is a fraction of the
     * selection composited over one of these two, so it lies between them.
     */
    for (const [ground, what] of [
      [palettes[theme]['--tk-bg'], 'its own background'],
      [palettes[theme]['--tk-sel'], 'a selection'],
    ]) {
      const dim = []
      for (const role of SCHEME_TOKENS) {
        const value = palettes[theme][propertyFor(role)]
        const ratio = contrast(value, ground)
        if (ratio !== null && ratio < INK_FLOOR) dim.push(`${role}=${value} ${ratio.toFixed(2)}:1`)
      }
      eq(dim, [], `every ${theme} role clears ${INK_FLOOR}:1 on ${what} (${ground})`)
    }

    /*
     * And selecting must not *cost* an ink its legibility — see `SELECTION_KEEP`. This is the
     * claim that survives a retune of the palette; the two floors above are the backstop.
     */
    const bg = palettes[theme]['--tk-bg']
    const sel = palettes[theme]['--tk-sel']
    const lost = []
    for (const role of SCHEME_TOKENS) {
      const value = palettes[theme][propertyFor(role)]
      const onBg = contrast(value, bg)
      const onSel = contrast(value, sel)
      if (onBg === null || onSel === null) continue
      const kept = onSel / onBg
      if (kept < SELECTION_KEEP) {
        lost.push(`${role}=${value} keeps ${(kept * 100).toFixed(0)}%`)
      }
    }
    eq(
      lost,
      [],
      `${theme}: selecting costs no role more than ${((1 - SELECTION_KEEP) * 100).toFixed(0)}% ` +
        `of its contrast (selection ${sel} is ${contrast(sel, bg)?.toFixed(2)}:1 off the background)`,
    )

    /*
     * And the selection is a ground rather than a repaint: too close to the background and
     * dragging across a region does nothing visible. Measured with `apart` and not `contrast`,
     * because the fix for the bug above was to make the selection *darker* — it is legible as a
     * selection through chroma, and a luminance ratio would have rated the fix as a regression.
     */
    const separation = apart(palettes[theme]['--tk-sel'], palettes[theme]['--tk-bg'])
    ok(
      separation !== null && separation > 60,
      `${theme}: the selection is distinguishable from the background (${separation?.toFixed(0)})`,
    )

    const gutter = palettes[theme]['--tk-gutter']
    const gutterRatio = contrast(gutter, palettes[theme]['--tk-bg'])
    ok(
      gutterRatio !== null && gutterRatio >= GUTTER_FLOOR,
      `${theme}: line numbers clear ${GUTTER_FLOOR}:1 (${gutterRatio?.toFixed(2)})`,
    )
  }
  /*
   * And the ground is not the ink. Measured separately from the loop because it is a different
   * claim: the loop asks whether a *role* is legible, this asks whether the scheme has a usable
   * text colour at all, which is the one thing `ColorScheme::normalise` cannot invent.
   */
  for (const theme of ['light', 'dark']) {
    const ratio = contrast(palettes[theme]['--tk-fg'], palettes[theme]['--tk-bg'])
    ok(ratio !== null && ratio >= 7, `${theme}: plain text is comfortably legible (${ratio})`)
  }

  // --- 4. the applier ----------------------------------------------------------------------

  const scheme = (id, polarity, colors = {}) => ({
    id,
    polarity,
    colors: { bg: '#101010', fg: '#eeeeee', ...colors },
  })
  const dark = scheme('night', 'dark', { keyword: '#ff0000' })
  const light = scheme('day', 'light')

  eq(schemeToApply([dark], BUILTIN_SCHEME, 'dark'), null, 'the builtin means "clear"')
  eq(schemeToApply([dark], 'night', 'dark')?.id, 'night', 'a matching scheme is applied')
  eq(
    schemeToApply([dark], 'night', 'light'),
    null,
    'a dark scheme is refused under the light theme — the guard against a hand-edited setting',
  )
  eq(
    schemeToApply([dark], 'gone', 'dark'),
    null,
    'an id naming nothing falls back to the builtin rather than painting half a scheme',
  )
  eq(
    schemeToApply([{ id: 'x', polarity: 'dark', colors: { fg: '#fff' } }], 'x', 'dark'),
    null,
    'a scheme with no background is refused — it would paint a transparent editor',
  )
  eq(schemeToApply([dark, light], 'day', 'light')?.id, 'day', 'the light list is searched too')

  /*
   * `applyScheme` must **clear the whole set** before writing. A partial write leaves a role from
   * the previous scheme behind, which is a buffer painted from two themes at once — and neither
   * scheme is wrong about the roles it does define, so nothing anywhere reports it.
   */
  const el = fakeElement()
  applyScheme(el, dark)
  eq(el.style.get('--tk-keyword'), '#ff0000', 'a role the scheme carries is written')
  eq(el.style.get('--tk-bg'), '#101010', 'the ground is written')
  eq(el.style.get('--tk-string'), undefined, 'a role the scheme omits is left to the cascade')
  applyScheme(el, scheme('other', 'dark', { string: '#00ff00' }))
  eq(el.style.get('--tk-keyword'), undefined, 'the previous scheme’s roles are cleared first')
  eq(el.style.get('--tk-string'), '#00ff00', 'the new scheme’s roles are written')
  applyScheme(el, null)
  eq(el.style.size(), 0, 'selecting the builtin clears every property, not just the ones it set')
  eq(
    el.removed.filter((p) => p === '--tk-keyword').length >= 2,
    true,
    'every role is removed on every apply, whether or not the scheme mentions it',
  )
  /*
   * Nothing outside the role set may be written. Rust drops unknown keys on the way in
   * (`ColorScheme::normalise`); this is the second half of that, so a value that reached the DOM
   * by some other path still cannot become a property nobody reads.
   */
  applyScheme(el, scheme('sneaky', 'dark', { 'editor.background': '#123456', keyword: '#abcdef' }))
  eq(el.style.get('--tk-editor.background'), undefined, 'an unknown key is not written')
  eq(el.style.get('--tk-keyword'), '#abcdef', 'a known one beside it still is')

  // --- 5. the repaint guard ----------------------------------------------------------------

  /*
   * The failure this pins is an import that *appears to do nothing*, and it is invisible from
   * every other angle: the file is written, the setting is saved, the event is broadcast, and
   * the window keeps painting the builtin.
   *
   * It happens when the two halves of an import — the settings patch naming the id, and the
   * `cide://schemes-changed` carrying the scheme itself — reach a window in that order. A guard
   * keyed on `theme:id` marks `dark:night` as painted while the id still resolves to nothing,
   * and the list arriving a moment later compares equal. Comparing the resolved *scheme* by
   * reference is what makes the second arrival a repaint.
   */
  const painted = { theme: 'dark', id: 'night', scheme: null }
  ok(schemeIsPainted(painted, { theme: 'dark', id: 'night', scheme: null }), 'an identical state is a no-op')
  ok(
    !schemeIsPainted(painted, { theme: 'dark', id: 'night', scheme: dark }),
    'the schemes list arriving after the setting that names it is a repaint',
  )
  ok(
    !schemeIsPainted(
      { theme: 'dark', id: 'night', scheme: dark },
      { theme: 'dark', id: 'night', scheme: scheme('night', 'dark', { keyword: '#00ff00' }) },
    ),
    're-importing under the same id is a repaint — the object is new even though the id is not',
  )
  ok(!schemeIsPainted(painted, { theme: 'light', id: 'night', scheme: null }), 'a theme switch repaints')
  ok(!schemeIsPainted(null, painted), 'the first paint of a window always happens')

  // --- 6. the picker's list ----------------------------------------------------------------

  const rows = schemeChoices(
    [
      { id: 'night', name: 'Night', polarity: 'dark' },
      { id: 'day', name: 'Day', polarity: 'light' },
    ],
    'dark',
  )
  eq(rows[0], { value: BUILTIN_SCHEME, label: 'cide' }, 'the builtin is always first')
  eq(rows.length, 2, 'only schemes of the theme on screen are offered')
  eq(rows[1]?.value, 'night', 'and it is the matching one')

  // --- 7. the error path, which is the one nobody looks at ---------------------------------

  /*
   * `CoreError` is a tagged enum, so a rejected `invoke` arrives as `{ kind, detail }` and
   * `String()` of it is `"[object Object]"` — which is what the import failure showed. The
   * sentence Rust composed, naming the file and what was looked for inside it, reached nobody.
   *
   * Pinned here because a failure path is only ever seen by the person it is failing, so nothing
   * about the happy path can catch it going wrong again.
   */
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/ipc/errorText.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )
  const { errorText } = await import(`file://${join(out, 'errorText.js')}`)

  eq(
    errorText({ kind: 'serde', detail: 'x.vsix contains no colour themes — C/C++ is …' }),
    'x.vsix contains no colour themes — C/C++ is …',
    'a tagged CoreError shows the sentence Rust wrote, not [object Object]',
  )
  eq(errorText({ kind: 'tabPinned' }), 'tabPinned', 'a payload-free tag shows its name')
  eq(errorText('plain'), 'plain', 'a string passes through')
  eq(errorText(new Error('boom')), 'boom', 'an Error shows its message')
  eq(errorText(null), 'null', 'and anything else is still something')
  ok(
    !/\[object Object\]/.test(errorText({ kind: 'io', detail: 'nope' })),
    'no rejected command can reach a user as [object Object]',
  )

  for (const surface of ['src/settings/ColorSchemeRow.tsx', 'src/settings/KeymapSection.tsx']) {
    const text = readFileSync(surface, 'utf8')
    ok(!/String\(e\)/.test(text), `${surface} does not stringify a rejection with String()`)
    ok(/errorText\(/.test(text), `${surface} renders a rejection through errorText`)
  }

  // --- 8. something actually calls it ------------------------------------------------------

  /*
   * The failure this catches is the whole feature being inert: every assertion above can pass
   * while no window ever calls `applyScheme`, and the symptom is a setting that saves, survives
   * a relaunch, and changes nothing on screen. `installThemeSync` is the one place a window's
   * palette is painted from — see its own note on why it is not an effect in `App`.
   */
  const useSettings = readFileSync('src/settings/useSettings.ts', 'utf8')
  ok(/applyScheme\(/.test(useSettings), '`applyScheme` is called when the settings change')
  ok(/schemeToApply\(/.test(useSettings), 'and it is given `schemeToApply`’s answer')
  ok(
    /colorSchemeDark|colorSchemeLight/.test(useSettings),
    'and the id it resolves is chosen by the theme — the setting is per polarity',
  )
  ok(
    /schemeIsPainted\(/.test(useSettings),
    'and the repaint guard is the checked one, not an inline `theme:id` comparison',
  )

  /*
   * **Every surface that mounts CodeMirror must tell it which polarity it is drawing in.**
   *
   * Nothing did, so every editor in the app was a light-mode CodeMirror — and its base theme is
   * written in terms of generated `&light`/`&dark` classes across view, autocomplete, lint,
   * search and merge. Most of it was masked by our own stylesheets; the selection was not, and it
   * shipped as a lavender block with the text invisible on it. A fourth surface added without
   * this line would reintroduce the whole family, and nothing else here would notice.
   */
  for (const surface of [
    'src/editor/EditorSurface.tsx',
    'src/panes/DiffPane.tsx',
    'src/panes/MergePane.tsx',
  ]) {
    const text = readFileSync(surface, 'utf8')
    ok(/polarityExtension\(/.test(text), `${surface} tells CodeMirror its polarity`)
    ok(/watchPolarity\(/.test(text), `${surface} keeps that polarity in step with the theme`)
  }

  if (failed > 0) {
    console.error(`\n${failed} failure(s) out of ${checks} checks`)
    process.exit(1)
  }
  console.log(
    `scheme: ok (${checks} checks, ${SCHEME_ROLES.length} roles across 4 files, 2 palettes)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

/**
 * The three `CSSStyleDeclaration` methods `applyScheme` uses, plus a log of the removals.
 *
 * A stub rather than jsdom: this file has one consumer of the DOM and adding a 3 MB dependency
 * to observe three method calls is the wrong trade. The removal log is what lets the "clears
 * everything, not just what it set" assertion be about behaviour rather than about the end state
 * — those two are the same for one apply and different for two in a row, which is the case.
 */
function fakeElement() {
  const map = new Map()
  const removed = []
  return {
    removed,
    style: {
      setProperty: (name, value) => map.set(name, value),
      removeProperty: (name) => {
        removed.push(name)
        map.delete(name)
      },
      get: (name) => map.get(name),
      size: () => map.size,
    },
  }
}

/**
 * Every declaration block containing no nested block.
 *
 * The same shape as `check-theme.mjs`'s, and deliberately a second copy rather than an import:
 * these scripts are standalone by design — CI enumerates them and runs each on its own — and a
 * shared helper module between them is one more thing that has to be right for either to run.
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

/** Base declarations first, then this theme's overrides — cascade order. */
function resolve(rules, theme, props) {
  const values = {}
  const apply = (rule) => {
    for (const prop of props) {
      const match = rule.body.match(new RegExp(`(?:^|[;{\\s])${prop}\\s*:([^;]+)`))
      if (match) values[prop] = match[1].trim()
    }
  }
  const named = (selector) =>
    [...selector.matchAll(/\[data-theme=['"]?([\w-]+)['"]?\]/g)].map((m) => m[1])
  const base = (selector) =>
    selector.split(',').some((p) => /^(:root|html)$/.test(p.trim()))
  for (const rule of rules) if (base(rule.selector)) apply(rule)
  for (const rule of rules) if (named(rule.selector).includes(theme)) apply(rule)
  return values
}

/**
 * How far apart two colours *look*, or `null` when either is not a hex colour.
 *
 * Thiadmer Riemersma's redmean weighted RGB distance, the same one `check-theme.mjs` uses for the
 * notice edges and copied here for that file's stated reason — these scripts are standalone and a
 * shared module between them is one more thing that has to be right for either to run.
 *
 * **Not `contrast`, and the distinction is what makes the selection assertion correct.** WCAG
 * contrast is a ratio of luminance, which asks whether ink can be read on a ground. Whether a
 * *fill* is visible is a different question, and the answer to the selection bug was to lower the
 * selection's luminance and let its chroma carry it — which a contrast ratio would have scored as
 * the selection getting worse.
 */
function apart(a, b) {
  const x = rgb(a)
  const y = rgb(b)
  if (x === null || y === null) return null
  const mean = (x[0] + y[0]) / 2
  const [dr, dg, db] = [x[0] - y[0], x[1] - y[1], x[2] - y[2]]
  return Math.sqrt((2 + mean / 256) * dr * dr + 4 * dg * dg + (2 + (255 - mean) / 256) * db * db)
}

/** The three channels of a hex colour as 0…255, or `null` for anything that is not one. */
function rgb(colour) {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec((colour ?? '').trim())
  if (!hex) return null
  const digits =
    hex[1].length === 3
      ? [...hex[1]].map((c) => c + c)
      : [hex[1].slice(0, 2), hex[1].slice(2, 4), hex[1].slice(4, 6)]
  return digits.map((d) => parseInt(d, 16))
}

/** WCAG contrast between two hex colours, or `null` when either is not one. */
function contrast(a, b) {
  const la = luminance(a)
  const lb = luminance(b)
  if (la === null || lb === null) return null
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

/** Relative luminance per WCAG 2.x, or `null` for anything that is not a hex colour. */
function luminance(colour) {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec((colour ?? '').trim())
  if (!hex) return null
  const digits =
    hex[1].length === 3
      ? [...hex[1]].map((c) => c + c)
      : [hex[1].slice(0, 2), hex[1].slice(2, 4), hex[1].slice(4, 6)]
  const channels = digits.map((d) => {
    const v = parseInt(d, 16) / 255
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2]
}
