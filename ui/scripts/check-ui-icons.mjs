/**
 * The fence around the vendored UI icon set, and around the app's return to drawn marks.
 *
 * # What it is guarding
 *
 * This app drew its icons as Unicode characters until M23 — roughly a hundred and thirty of
 * them across forty files — and `chrome/ActivityRail.tsx` records why that could never be made
 * to work: what a character puts on screen is its ink inside its em box, and that ratio belongs
 * to whichever face fontconfig picked, not to us. Measured on this host, seven rail glyphs
 * spanned 0.53em to 0.82em of ink. The rail was converted to paths and the argument stopped
 * there; everything else kept drawing characters, so `⑂` shipped in the branch indicator
 * despite a note two files away saying no UI font carries it, `↻` meant two different actions
 * in one toolbar, and `×` and `✕` both meant close in the same file.
 *
 * The risky part is not the conversion. It is the next component: one `<span>✓</span>` written
 * in good faith by somebody who has never read this file, on a host where that glyph happens to
 * render, in a codebase where a hundred sibling comments still quote the old characters.
 *
 * # The five things it asserts
 *
 *   1. **Every name the source draws exists in the generated set.** A missing name renders an
 *      empty `<path>`: no error, no log, an empty box, and invisible in review.
 *   2. **No dead entries.** Unlike `public/icons/`, where an unused Material icon is a file
 *      nobody fetches, an unreferenced entry here is bytes in the JS bundle that every window
 *      parses on every launch. Drop it from `ICONS` in `vendor-ui-icons.mjs` and re-run.
 *   3. **Every mark is a legal *extension* icon.** The vendored set and an extension's
 *      contributed `d` have to be the same kind of value, or `<Icon>` needs two render paths
 *      and the one exercised only by untrusted input becomes the least-tested one. This runs a
 *      port of `cide_ext::manifest::is_svg_path` over every generated path.
 *   4. **The stroke stays in band at every size.** A 24-unit grid at a fixed `stroke-width`
 *      renders 2px at 24px and 1.08px at 13px, which is the 55% spread the app shipped with
 *      before the sizes were pitched against each other. Below ~1.4px WebKit stops resolving a
 *      line and starts antialiasing a smear.
 *   5. **No rendered Unicode symbol survives** outside a per-file allowlist of things that are
 *      genuinely text.
 *
 * # How (5) avoids firing on prose
 *
 * By walking the TypeScript AST and visiting **only** string literals, template parts and JSX
 * text. Comments are never visited, which removes the entire false-positive class at a stroke:
 * this repository discusses its own glyphs at length — the rail's ink table, the twisty
 * arithmetic, the merge gutter's reasoning — and a gate that fired on that would be switched
 * off within a week. `node_modules/typescript` is already a dependency of every check here, so
 * there is no new tool and no hand-rolled comment stripper to get wrong on a `//` inside a
 * regex literal.
 *
 * Run: `pnpm --dir ui run check:ui-icons`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, relative, resolve } from 'node:path'
import ts from 'typescript'

const SRC = resolve('src')
let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

/* -- the generated set --------------------------------------------------------------------- */

const generated = readFileSync(join(SRC, 'icons/iconPaths.ts'), 'utf8')
const PATHS = new Map(
  [...generated.matchAll(/^ {2}'([\w-]+)': '([^']*)',/gm)].map((m) => [m[1], m[2]]),
)
ok(PATHS.size > 0, 'iconPaths.ts holds a set at all — a parse that finds nothing passes vacuously')

/**
 * The JavaScript half of `cide_ext::manifest::is_svg_path`.
 *
 * Restated rather than imported: the authority is Rust, and this is the mirror whose whole job
 * is to prove the generated set lives inside the space an extension's icon has to live in. The
 * vendor script runs the same predicate before writing; this runs it on the committed file, so
 * a hand-edit is caught even though the script is only run by hand.
 */
const isSvgPath = (d) =>
  d.length > 0
  && d.length <= 4096
  && /^[A-Za-z0-9.,\-+ \t\n\r]*$/.test(d)
  && [...d].filter((c) => /[A-Za-z]/.test(c)).every((c) => 'MmLlHhVvCcSsQqTtAaZz'.includes(c))

for (const [name, d] of PATHS) {
  ok(isSvgPath(d), `\`${name}\` is a value an extension could also have supplied (is_svg_path)`)
}

/* -- what the source draws ----------------------------------------------------------------- */

function sources(dir, ext) {
  const out = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...sources(path, ext))
    else if (ext.some((e) => entry.name.endsWith(e))) out.push(path)
  }
  return out
}

/**
 * Two sets, because there are two kinds of evidence.
 *
 * **Precise** — `<Icon name="…">`, `iconElement('…')`, `asIcon('…')`, and a CSS `var(--icon-…)`.
 * Every one of these is unambiguously a mark, so a name here that is not in the set is a typo
 * and a failure.
 *
 * **Loose** — any string literal, in a file that imports the icon API, that happens to equal a
 * name in the set. This exists only to keep the dead-entry check honest: a mark can reach
 * `<Icon>` through a `const`, a `Record`, a prop called `icon` or a helper called `button`, and
 * enumerating those shapes syntactically is a losing game. Filtering to known names means a
 * typo *cannot* be caught this way, which is exactly why the precise set is separate.
 */
const referenced = new Set()
const loose = new Set()
/** Every non-ASCII symbol actually rendered, as `file -> Set<char>`. */
const symbols = new Map()

/**
 * Characters that are prose, everywhere, and never an icon.
 *
 * `·` is here only when it is *spaced* — see the check below. Spaced, it is the separator this
 * app uses between clauses in a status line; unspaced, it was a bullet standing in for a mark.
 */
const PROSE = new Set(['—', '–', '…', '’', '‘', '“', '”', '‑', ' ', '×', '·', '→', '←', '›', '−', '±'])

/**
 * Per-file exemptions, each with the reason it is not an icon.
 *
 * Keyed on file **and** character, so `⋯` can be text in the commit graph and still a failure in
 * the branch indicator. A dead entry is itself a failure: `check-theme.mjs` states the principle
 * — "an exemption for a slot that would pass anyway is a dead one, and a dead exemption is how
 * the next regression walks past this gate".
 */
const ALLOWED = [
  {
    file: 'keys/chords.ts',
    chars: '⌃⌥⇧⌘↑↓⏎⇥␣⌫⌦',
    why: 'Key caps. The chip is a fixed badge at the right edge of a 620px row, and the spelled '
      + 'out form is more than twice the width of the widest symbol — the file says so above '
      + 'MODIFIER_SYMBOLS. They compose (⌃⇧P), they are what is printed on the keyboard, and '
      + 'they have to line-break and select as text.',
  },
  {
    file: 'keys/switcher.ts',
    chars: '↑↓',
    why: 'KEY_CAPS — the same argument as chords.ts.',
  },
  {
    file: 'overlays/CommandPalette.tsx',
    chars: '↑↓⏎⌃',
    why: '`<Hint keys="↑↓">navigate</Hint>` — a key hint in a mono footer chip. `ModalShell.tsx` '
      + 'states the rule: the glyph is mono and dim, the words are not.',
  },
  {
    file: 'overlays/FilePicker.tsx',
    chars: '↑↓⏎⇧⌥',
    why: 'Key hints in the footer — see CommandPalette.',
  },
  { file: 'overlays/SymbolPicker.tsx', chars: '↑↓⏎', why: 'Key hints in the footer.' },
  { file: 'kit/page/chapters/Overlays.tsx', chars: '↑↓⏎', why: 'The PickerHint specimen: key hints in a picker footer, as the app pickers draw them.' },
  { file: 'overlays/StructurePicker.tsx', chars: '↑↓⏎', why: 'Key hints in the footer.' },
  { file: 'overlays/UsagesPopup.tsx', chars: '↑↓⏎', why: 'Key hints in the footer.' },
  {
    file: 'demo/transcripts.ts',
    chars: '↓⏺⎿▐▛█▜▌▝▘─✻⏵',
    why: 'Terminal output for the screenshot demo (M103): Claude Code\'s own glyphs — the logo '
      + 'blocks, the ⏺ tool rows, ⎿ results, the ✻ spinner, the rules — written as bytes into '
      + 'xterm. Text in a terminal, never a mark in cide\'s chrome.',
  },
  { file: 'demo/data/editor.ts', chars: '⸢⸣', why: 'Demo source text shown in the editor (M103) — file content, not chrome.' },
  { file: 'demo/data/log.ts', chars: '«»', why: 'Demo commit messages (M103) — prose a commit author typed, not a mark.' },
  { file: 'demo/data/milestones.ts', chars: '✓✗', why: 'Demo verify output and task text (M103) — content the board displays, not a mark it draws.' },
  {
    file: 'panes/GitDiffPane.tsx',
    chars: '⏎⋯',
    why: 'The ⏎ is a key hint beside a staging control. The ⋯ opens the fold row\'s label — '
      + '"⋯ 5,998 unchanged lines" — the same glyph every editor puts on a folded region, '
      + 'inside a sentence that line-breaks and selects as text; an icon would divorce the '
      + 'mark from the count it qualifies.',
  },
  {
    file: 'chrome/branchModel.ts',
    chars: '↑↓',
    why: 'The ahead/behind summary *sentence* — a title and an accessible name, not a control. '
      + 'The branch selector draws the same two counts as marks; this is the text a screen '
      + 'reader gets, and an icon has nothing to say to one.',
  },
  {
    file: 'editor/blameModel.ts',
    chars: '↳',
    why: 'The blame popup\'s "this line came from" continuation, inside a sentence.',
  },
  {
    file: 'gitlog/LogView.tsx',
    chars: '⋯',
    why: "GraphCell's lane-bundle label, absolutely positioned inside a 10px lane column. Any "
      + 'mark is wider than the lane it would have to sit in.',
  },
  {
    file: 'panes/RevisionPane.tsx',
    chars: '⌫',
    why: '`⌫ Back` — a key hint, matching chords.ts.',
  },
]

for (const file of sources(SRC, ['.ts', '.tsx'])) {
  const rel = relative(SRC, file)
  const text = readFileSync(file, 'utf8')
  const sf = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TSX)

  const note = (raw) => {
    for (const ch of raw) {
      if (ch.codePointAt(0) <= 0x7f) continue
      if (/[\p{L}\p{N}]/u.test(ch)) continue
      if (PROSE.has(ch)) continue
      if (!symbols.has(rel)) symbols.set(rel, new Set())
      symbols.get(rel).add(ch)
    }
  }

  /*
   * `AgentsPanel/model.ts` and `TasksPanel/model.ts` are named rather than detected, and the
   * reason is the same one their own headers give: they are **import-free on purpose**, because
   * `check-agents.mjs` compiles each standalone with no `--rootDir` and imports the output
   * directly. They cannot import the icon API even for a type, so their phase and status tables
   * hold plain strings and `asIcon` narrows at the render site. Without these two lines every
   * mark only they name reads as dead. `markdownTools.ts` is the third of the family — the
   * formatting toolbar's tool table, import-free for the same check.
   */
  const TABLE_FILES = [
    'sidebar/AgentsPanel/model.ts',
    'sidebar/TasksPanel/model.ts',
    'sidebar/TasksPanel/markdownTools.ts',
  ]
  const usesIcons =
    TABLE_FILES.includes(rel) || /from '(@\/icons[\w/]*|\.\.?\/[\w/.]*icons?[\w/]*)'/.test(text)

  const walk = (node) => {
    // --- names the app draws -------------------------------------------------------------
    if (
      ts.isJsxAttribute(node)
      && node.name.getText() === 'name'
      && node.initializer
      // On an `<Icon>` and nothing else: `name` is an ordinary prop name, and a rail item, a
      // settings row and an agent role all have one.
      && node.parent?.parent?.tagName?.getText() === 'Icon'
    ) {
      const init = node.initializer
      const lit = ts.isStringLiteral(init)
        ? init
        : ts.isJsxExpression(init) && init.expression && ts.isStringLiteral(init.expression)
          ? init.expression
          : null
      if (lit) referenced.add(lit.text)
      // `name={cond ? 'a' : 'b'}` — the shape every twisty uses.
      if (ts.isJsxExpression(init) && init.expression && ts.isConditionalExpression(init.expression)) {
        for (const branch of [init.expression.whenTrue, init.expression.whenFalse]) {
          if (ts.isStringLiteral(branch)) referenced.add(branch.text)
        }
      }
    }
    if (ts.isCallExpression(node) && /^(iconElement|asIcon)$/.test(node.expression.getText())) {
      const [first] = node.arguments
      if (first && ts.isStringLiteral(first)) referenced.add(first.text)
    }
    // --- symbols actually rendered ---------------------------------------------------------
    if (
      ts.isStringLiteral(node)
      || ts.isNoSubstitutionTemplateLiteral(node)
      || ts.isTemplateHead(node)
      || ts.isTemplateMiddle(node)
      || ts.isTemplateTail(node)
      || ts.isJsxText(node)
    ) {
      note(node.text)
      if (usesIcons && PATHS.has(node.text)) loose.add(node.text)
    }
    ts.forEachChild(node, walk)
  }
  walk(sf)
}

// CSS reaches the set through the generated mask file rather than through `<Icon>`.
const masks = readFileSync(join(SRC, 'styles/iconMasks.css'), 'utf8')
const declaredMasks = new Set([...masks.matchAll(/--icon-([\w-]+):/g)].map((m) => m[1]))
const usedMasks = new Set()
for (const file of sources(SRC, ['.css'])) {
  if (relative(SRC, file) === 'styles/iconMasks.css') continue
  for (const m of readFileSync(file, 'utf8').matchAll(/var\(--icon-([a-z][\w-]*)\)/g)) {
    // `--icon-0` … `--icon-3` are box sizes in tokens.css, not marks.
    if (/^\d/.test(m[1])) continue
    usedMasks.add(m[1])
  }
}
for (const name of usedMasks) {
  ok(declaredMasks.has(name), `\`--icon-${name}\` is emitted into iconMasks.css by the vendor script`)
  referenced.add(name)
}
for (const name of declaredMasks) {
  ok(usedMasks.has(name), `\`--icon-${name}\` has a reader — a mask nothing draws is dead weight`)
}

/* -- 1 and 2: the set is exactly what the app draws ----------------------------------------- */

const missing = [...referenced].filter((n) => !PATHS.has(n)).sort()
eq(
  missing,
  [],
  'every mark the source names is in the generated set. A name that is not renders an empty '
    + '`<path>`: no error, no log, an empty box, and nothing in a screenshot of a state that '
    + 'happens not to be showing it',
)
const dead = [...PATHS.keys()].filter((n) => !referenced.has(n) && !loose.has(n)).sort()
eq(
  dead,
  [],
  'and nothing is vendored that the app never draws — an unreferenced entry is bytes every '
    + "window parses on every launch. Remove it from `ICONS` in `vendor-ui-icons.mjs` and re-run",
)

/* -- 4: the stroke stays in band ------------------------------------------------------------ */

/*
 * The box and the stroke reach the mark at all.
 *
 * This gate used to check only the arithmetic in the stylesheet, and the arithmetic was right
 * while the rules reached nothing: they lived in `Icon.module.css` under a hashed class that
 * `<Icon>` applied and `iconElement` — which builds the merge gutter's buttons, the find bar's
 * and the fold gutter's, inside CodeMirror where no hashed name exists — did not. `data-size`
 * on its own selects nothing, an `<svg>` with no width fills its container, and
 * `.cm-mergeChevron` is `width: 100%`. The marks came out the size of the gutter and shoved the
 * ignore button out of the row.
 *
 * So: one literal class, written by both builders, defined in a plain stylesheet. A module
 * import here would reintroduce exactly the split that caused it.
 */
const iconCss = readFileSync(join(SRC, 'icons/icon.css'), 'utf8')
const iconElementTs = readFileSync(join(SRC, 'icons/iconElement.ts'), 'utf8')
const iconTsx = readFileSync(join(SRC, 'icons/Icon.tsx'), 'utf8')

const literal = /export const ICON_CLASS = '([\w-]+)'/.exec(iconElementTs)?.[1]
ok(literal !== undefined, '`ICON_CLASS` is a literal string in `icons/iconElement.ts`')
if (literal !== undefined) {
  ok(
    iconCss.includes(`.${literal} {`),
    `\`icons/icon.css\` defines \`.${literal}\` — the class both builders write`,
  )
  ok(
    new RegExp(`\\.${literal}\\[data-size=`).test(iconCss),
    'and sizes it per `data-size`, which is the attribute both builders set',
  )
}
ok(
  /svg\.setAttribute\('class'/.test(iconElementTs)
    && !/className === undefined \? '' :/.test(iconElementTs),
  '`iconElement` applies the sizing class unconditionally rather than leaving it to a caller — '
    + 'an optional class is an optional size, and a mark with no size fills its container',
)
ok(
  iconTsx.includes('ICON_CLASS'),
  '`<Icon>` writes the same literal, so the component and the imperative builder cannot drift',
)
const importsModule = (src) => /^\s*import .*\.module\.css'/m.test(src)
ok(
  !importsModule(iconTsx) && !importsModule(iconElementTs),
  'neither builder imports a CSS module for the box — a hashed name is unreachable from the '
    + 'CodeMirror surfaces, which is how the two paths came apart',
)

/*
 * And the imperative builder really does apply it — **run**, not grepped.
 *
 * The first version of this assertion was a regex for `setAttribute('class'`, and it passed
 * against the exact bug it was written for: the call was still there, wrapped in
 * `if (className !== undefined)`. A pattern that matches the broken code as happily as the
 * fixed one is worse than no pattern, because it reads like cover. `iconElement.ts` imports
 * nothing but `iconPaths.ts`, so it compiles standalone and can simply be called.
 */
{
  const out = mkdtempSync(join(tmpdir(), 'cide-ui-icons-'))
  try {
    execFileSync(
      'node',
      [
        'node_modules/typescript/bin/tsc',
        'src/icons/iconElement.ts',
        '--outDir', out,
        '--rootDir', 'src',
        '--module', 'commonjs',
        '--moduleResolution', 'node10',
        '--target', 'es2022',
        '--strict',
        '--noUncheckedIndexedAccess',
        '--exactOptionalPropertyTypes',
      ],
      { stdio: 'inherit' },
    )
    // The smallest DOM `iconElement` touches: two namespaced elements and `append`.
    const made = []
    globalThis.document = {
      createElementNS(_ns, tag) {
        const el = { tag, attrs: {}, children: [],
          setAttribute(k, v) { this.attrs[k] = v },
          append(...c) { this.children.push(...c) } }
        made.push(el)
        return el
      },
    }
    const { iconElement, ICON_CLASS } = await import(
      `file://${join(out, 'icons/iconElement.js')}`
    )
    for (const size of [0, 1, 2, 3]) {
      const el = iconElement('x', size)
      eq(
        el.attrs.class,
        ICON_CLASS,
        `iconElement('x', ${size}) carries the sizing class. Without it \`data-size\` selects `
          + 'nothing, the `<svg>` has no width, and it fills whatever contains it',
      )
      eq(String(el.attrs['data-size']), String(size), 'and the rung it was asked for')
    }
    const withExtra = iconElement('x', 1, 'cm-mergeChevron')
    ok(
      withExtra.attrs.class.split(/\s+/).includes(ICON_CLASS),
      'a caller-supplied class is added to the sizing class rather than replacing it — '
        + 'replacing it is the bug, in the exact shape it shipped',
    )
  } finally {
    rmSync(out, { recursive: true, force: true })
    delete globalThis.document
  }
}

/* -- 5: nothing is still drawing a character ------------------------------------------------ */

for (const [file, chars] of [...symbols].sort()) {
  const entry = ALLOWED.find((a) => a.file === file)
  const spare = [...chars].filter((c) => entry === undefined || !entry.chars.includes(c))
  ok(
    spare.length === 0,
    `${file} draws no Unicode symbol as an icon (found ${spare.join(' ')}). Marks come from `
      + '`<Icon>`; if these are genuinely text, add a `why` to ALLOWED in this script',
  )
}
for (const entry of ALLOWED) {
  const drawn = symbols.get(entry.file)
  const unused = [...entry.chars].filter((c) => drawn === undefined || !drawn.has(c))
  eq(
    unused,
    [],
    `every character allowed in ${entry.file} is still drawn there — a dead exemption is how `
      + 'the next regression walks past this gate',
  )
}

/* -- 6: a mark's box is not displaced by the padding nobody reset ---------------------------- */

/*
 * **`appearance: none` does not reset a `<button>`'s padding**, and WebKit's default is `1px 6px`.
 *
 * With the global `box-sizing: border-box`, a `width: 20px` icon button therefore has an **8px**
 * content box. The mark's auto-sized grid track is 14px, a track wider than its content box
 * overflows towards the inline *end*, and the glyph ends up 6px from the left edge of its own
 * hover background and flush against the right. `place-items: center` cannot save it: that
 * centres the item inside its track, and it is the track that is displaced.
 *
 * Reported as "the icon hover background isn't centred", which is exactly what it looks like.
 * Every icon button written before the Docker panel already set `padding: 0` — `ActivityRail`,
 * `TabStrip`, the tool window — so this had never been seen. A convention that four files keep
 * and the fifth forgets is not a rule, which is what this turns it into.
 *
 * The rule is deliberately narrow: a block that both centres with `place-items: center` **and**
 * fixes a `width` is an icon-sized box, and one that neither states a padding nor composes from
 * something that does is the shape that fails. A padded button (a toolbar row, a labelled
 * control) states its padding and is not matched.
 */
{
  const styles = sources(SRC, ['.css'])
  for (const file of styles) {
    const text = readFileSync(file, 'utf8')
    const where = relative(SRC, file)
    // Rule blocks, crudely but sufficiently: this stylesheet dialect nests nothing but media
    // queries, and a media query's body has no `place-items` of its own.
    for (const [, selector, body] of text.matchAll(/([^{}]*)\{([^{}]*)\}/g)) {
      if (!/place-items:\s*center/.test(body)) continue
      if (!/\bwidth:/.test(body)) continue
      if (/\bpadding\b/.test(body) || /\bcomposes:/.test(body)) continue
      ok(
        false,
        `${where}: \`${selector.trim().split('\n').pop().trim()}\` centres a mark in a fixed-width `
          + 'box and resets no padding. If it is a <button>, WebKit keeps `1px 6px` through '
          + '`appearance: none` and the mark lands off its own hover background — add '
          + '`padding: 0`. If it is not a button, say so with `padding: 0` anyway or give it the '
          + 'padding it means.',
      )
    }
  }
}

if (failed) {
  console.error(`\n${failed} icon check(s) failed`)
  process.exit(1)
}
console.log(
  `ui icons: ok (${PATHS.size} marks, ${declaredMasks.size} masks, ${ALLOWED.length} text exemptions)`,
)
