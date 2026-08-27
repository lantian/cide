/**
 * Re-vendors the UI icon set into `src/icons/iconPaths.ts` from upstream Lucide.
 *
 * Maintenance only — this is not part of the build, and it needs network access. Run it to move
 * the set forward, or after editing {@link ICONS} below, which is the whole of "which marks does
 * this app draw":
 *
 *     node ui/scripts/vendor-ui-icons.mjs     # from the repository root
 *     pnpm --dir ui run check:ui-icons        # then always this
 *
 * Source: https://github.com/lucide-icons/lucide, ISC. The licence is copied to
 * `public/LUCIDE-LICENSE` by this script so it travels with the artwork — `public/` is copied
 * verbatim into `dist`, which is the same route `public/icons/LICENSE` takes for the Material
 * file-icon set. It is **not** kept beside the generated module: a bang-banner legal comment in
 * the source would be the obvious place, and it does not survive this build — see the note above
 * the banner below.
 *
 * # Why an icon set at all, after three files said there would not be one
 *
 * `chrome/ActivityRail.tsx`, `sidebar/Explorer.tsx` and `sidebar/GitPanel/Toolbar.tsx` each
 * stated "this application bundles no UI icon set" as policy rather than as a gap. That policy
 * is reversed here, and the evidence that reversed it was already written down in the rail's own
 * header: the rail was seven Unicode characters at seven font sizes, reported twice — first as
 * "icons are quite small", then as "why all icons of different size? They should be one size and
 * bigger" — and the measurement that followed showed why no font size could fix it. What a
 * character puts on screen is its ink inside its em box, and that ratio belongs to whichever face
 * fontconfig picked: 0.53em to 0.82em across seven glyphs, so `⚙` drew nearly twice the mark `≡`
 * did while their font sizes differed by 3px.
 *
 * The rail was fixed by drawing eight paths by hand, and its comment then argued *against*
 * converting anything else, on the grounds that half glyphs and half paths is two icon systems to
 * keep matched rather than one. That argument pointed the other way as soon as the rest of the
 * app — the git toolbar, the pane title bar, the status bar, six panels, every dialog — stayed on
 * characters. `⑂` (U+2442) ships in the branch indicator despite the rail's own note that it is
 * in no UI font; `↻` means two different actions in one toolbar; `×` and `✕` both mean close in
 * the same file. One system is the answer, and it was already sitting in `RailIcon`.
 *
 * # Why Lucide
 *
 * Because it is the envelope this app already draws. `RailIcon` renders `viewBox="0 0 24 24"`,
 * `fill="none"`, `stroke="currentColor"`, `stroke-width="2"`, round cap and join — which is
 * Lucide's own, to the attribute. The eight hand-drawn paths were made to that convention, so
 * adopting the set extends the existing system rather than replacing it, and nothing on the rail
 * has to change to match. ISC, a Feather fork, and vendored as **data** rather than as a
 * dependency: no runtime package, no tree-shaking question, one generated file to review.
 *
 * # Why every icon is flattened to a single `d`
 *
 * This app already publishes exactly one canonical icon value, and it is fixed in Rust: an
 * extension contributes a rail icon as a 24x24 path string, whitelisted by
 * `cide_ext::manifest::is_svg_path`. If the built-in set were richer — element lists, or
 * `{tag, attrs}` tuples — then `<Icon>` would need two render paths, the one exercised *only* by
 * extension data would be the least-tested one, and a contributed icon could never look like a
 * built-in. Flattening makes the most-tested path the only path and yields an invariant the
 * gate can state: every mark this app draws, vendored or contributed, is the same kind of value.
 *
 * It is not a compromise. Measured across the whole upstream set, the geometry uses seven element
 * types and the conversions below are lossless for a stroke-only icon: a circle and an ellipse
 * become two arcs, a rect becomes four lines or four lines and four arcs, and a line, polyline or
 * polygon becomes `M`/`L`. The seam a closed circle introduces sits under `stroke-linejoin:
 * round` and is invisible. Two things make that total rather than usually-true, and both are
 * assertions in this script: the envelope is checked on every file, and **any icon carrying a
 * `fill` on a child element is refused by name** — a filled mark is the one class where dropping
 * to stroke-only would silently lose ink.
 */
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tmpdir } from 'node:os'

const UI = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT_TS = join(UI, 'src/icons/iconPaths.ts')
const OUT_LICENCE = join(UI, 'public/LUCIDE-LICENSE')

const UPSTREAM = 'https://github.com/lucide-icons/lucide.git'
/**
 * The revision `iconPaths.ts`'s header names, and the one this script fetches.
 *
 * Pinned rather than `clone --depth 1` of whatever the default branch happens to be, for
 * `vendor-icons.mjs`'s reason and it is the same reason: the header is a provenance claim —
 * "these marks are upstream's, at this commit" — and a script that records HEAD *after* cloning
 * cannot reproduce the file it just stamped. The next run would silently re-vendor against a
 * different upstream under a commit message saying nothing changed. Moving the set forward is a
 * deliberate edit to this line. `1.33.0`.
 */
const UPSTREAM_REV = '59978cecf84986af59f1f9f503bcebdc89c6d166'

/**
 * Every mark this app draws, and nothing else.
 *
 * A closed list rather than the whole 1776-icon set: an unreferenced entry here is bytes in the
 * JS bundle that every window parses on every launch, which is the opposite trade from
 * `public/icons/`, where an unused Material icon is a file nobody fetches. `check-ui-icons.mjs`
 * fails on a dead entry for that reason.
 *
 * Grouped by where they are drawn, so a reader can tell whether a name is still needed.
 */
const ICONS = [
  // Activity rail.
  'file-text', 'git-branch', 'search', 'triangle-alert', 'hexagon', 'square-check-big',
  'blocks', 'settings', 'panel-bottom', 'puzzle',
  // Git tool window toolbar. `refresh-cw` and `arrow-down-to-line` are the two actions that
  // shared `↻` because there was no icon set to tell them apart; there is now.
  'refresh-cw', 'undo-2', 'arrow-down-to-line', 'plus', 'archive', 'columns-2', 'circle-dot',
  'chevrons-down-up', 'chevrons-up-down',
  // Pane title bar and window controls. `move` is the grab handle: press it and drag the pane
  // into another row or column.
  'minus', 'square', 'x', 'square-plus', 'maximize', 'picture-in-picture-2', 'move',
  // Chevrons, everywhere: menus, twisties, overflow, find bar, merge navigation.
  'chevron-down', 'chevron-left', 'chevron-right', 'chevron-up',
  'chevrons-left', 'chevrons-right',
  // Status bar and branch indicator. `git-branch` replaces `⑂` (U+2442), which the rail's own
  // header records as being in no UI font. `circle-plus` left with the status bar's diff stat
  // (M28); `circle-minus` stayed because an interrupted agent run draws it.
  'circle-minus', 'circle-x', 'arrow-up', 'arrow-down', 'ellipsis',
  // App header.
  'sun-moon', 'rows-2',
  // Tool window.
  'panel-bottom-close',
  // Context menu, checkboxes, confirmations.
  'check',
  // Problems, and the editor's lint gutter, which draws the same alphabet.
  'info',
  // Agent phases: one silhouette, eight interiors — which is what `○ ◌ ● ◉` was reaching for.
  'circle', 'circle-dashed', 'loader-circle', 'circle-alert', 'circle-pause', 'circle-check',
  'circle-slash',
  // Agent and task controls. Deliberately outside the circle family so a status and a button
  // never look alike.
  'pause', 'play', 'eye', 'trash-2', 'pencil',
  // Extension-contributed panel rows.
  'file', 'folder', 'diamond',
  // Explorer header. `asterisk` is a footnote mark: the truncation notice is a footnote, not an
  // error, which is what its own comment insists on.
  'asterisk', 'crosshair',
  // Git guard and merge bars.
  'swords',
  // Search options. These three exist upstream because they are VS Code's three find toggles,
  // which is exactly what the literal strings "Aa", "ab" and ".*" were standing in for.
  'case-sensitive', 'whole-word', 'regex',
  // Switcher and dialogs.
  'file-diff', 'message-square', 'arrow-up-right', 'arrow-right', 'arrow-left-right',
  // Git log and the markdown view switcher.
  'list-tree', 'file-code', 'book-open-text',
  // The task composer's formatting toolbar — `sidebar/TasksPanel/markdownTools.ts`. `text-quote`
  // rather than `quote`: upstream's `quote` is a filled speech mark, and filled icons are
  // refused below by design.
  'bold', 'italic', 'code', 'link', 'heading', 'text-quote', 'list', 'list-ordered',
]

/**
 * The icons CSS has to draw through `mask-image`, because their site is a `content:` rule.
 *
 * The CodeMirror lint gutter and the markdown preview's external-link marker are stylesheets,
 * not components, so `<Icon>` cannot reach them. Emitted as data URIs into a second generated
 * file; see the bottom of this script for why the stroke is baked to black there.
 */
const CSS_ICONS = ['circle-x', 'triangle-alert', 'info', 'minus', 'arrow-up-right']

// ---------------------------------------------------------------------------------------
// Fetch upstream at the pinned revision.
// ---------------------------------------------------------------------------------------

const CHECKOUT = join(tmpdir(), `cide-ui-icons-${Math.random().toString(36).slice(2, 10)}`)
mkdirSync(CHECKOUT, { recursive: true })
const git = (...args) => execFileSync('git', ['-C', CHECKOUT, ...args], { stdio: ['ignore', 'pipe', 'inherit'] })

try {
  git('init', '-q', '.')
  git('remote', 'add', 'origin', UPSTREAM)
  git('fetch', '-q', '--depth', '1', 'origin', UPSTREAM_REV)
  git('checkout', '-q', 'FETCH_HEAD')
  const head = git('rev-parse', 'HEAD').toString().trim()
  if (head !== UPSTREAM_REV) {
    throw new Error(`fetched ${head}, expected ${UPSTREAM_REV} — the pin above is the claim this file makes`)
  }

  // -------------------------------------------------------------------------------------
  // Parse, assert the envelope, flatten.
  // -------------------------------------------------------------------------------------

  /** The six attributes that make a Lucide icon the thing `RailIcon` already draws. */
  const ENVELOPE = [
    ['viewBox', '0 0 24 24'], ['fill', 'none'], ['stroke', 'currentColor'],
    ['stroke-width', '2'], ['stroke-linecap', 'round'], ['stroke-linejoin', 'round'],
  ]

  const num = (v) => {
    const n = Number(v)
    if (!Number.isFinite(n)) throw new Error(`not a number: ${v}`)
    // Trim float noise from the arc arithmetic without losing real precision.
    return Number(n.toFixed(4))
  }

  /**
   * A path's data with its opening move made absolute, leaving everything after it alone.
   *
   * This is the whole correctness of concatenation, and it is not obvious. Most Lucide paths open
   * with a *relative* `m` — `m19 12-7 7-7-7` — which SVG treats as absolute **only because it is
   * the first command in its own `d`**. Append that string after another subpath and the rule no
   * longer applies: the move becomes relative to wherever the previous subpath ended, and the
   * mark is drawn in the wrong place. `arrow-down` came out with its head displaced by (12, 5)
   * exactly this way, and nothing about the output looks malformed — it is a valid path that
   * draws the wrong picture.
   *
   * Uppercasing the `m` is not the fix either, and this is the trap behind the trap: the command
   * letter also sets the implicit operator for every coordinate pair that follows it. Pairs after
   * `m` are relative linetos, pairs after `M` are absolute ones, so `m19 12-7 7-7-7` and
   * `M19 12-7 7-7-7` are different drawings. So the move alone is promoted and the tail is handed
   * an explicit `l` to keep it relative.
   */
  function absolutise(d) {
    const m = /^\s*m\s*(-?[\d.]+)[\s,]+(-?[\d.]+)(.*)$/s.exec(d)
    if (!m) return d.trim()
    const rest = m[3].trim()
    // A bare `m x y` with no tail needs no `l`, and would be invalid with one.
    return `M${num(m[1])} ${num(m[2])}${rest && !/^[A-Za-z]/.test(rest) ? `l${rest}` : rest}`
  }

  /**
   * One SVG element as path data.
   *
   * A circle and an ellipse become two half arcs closed with `Z`; a rect becomes four lines, or
   * four lines and four corner arcs when it is rounded; a line, polyline and polygon become
   * `M`/`L`. Every one is exact for a stroke-only mark.
   */
  function toPath(tag, a) {
    const ellipse = (cx, cy, rx, ry) =>
      `M${num(cx - rx)} ${num(cy)}`
      + `a${num(rx)} ${num(ry)} 0 1 0 ${num(rx * 2)} 0`
      + `a${num(rx)} ${num(ry)} 0 1 0 ${num(-rx * 2)} 0Z`
    const points = (raw, close) => {
      const nums = raw.trim().split(/[\s,]+/).map(num)
      let d = ''
      for (let i = 0; i + 1 < nums.length; i += 2) d += `${i ? 'L' : 'M'}${nums[i]} ${nums[i + 1]}`
      return close ? `${d}Z` : d
    }
    switch (tag) {
      case 'path':
        return absolutise(a.d)
      case 'circle':
        return ellipse(num(a.cx), num(a.cy), num(a.r), num(a.r))
      case 'ellipse':
        return ellipse(num(a.cx), num(a.cy), num(a.rx), num(a.ry ?? a.rx))
      case 'line':
        return `M${num(a.x1)} ${num(a.y1)}L${num(a.x2)} ${num(a.y2)}`
      case 'polyline':
        return points(a.points, false)
      case 'polygon':
        return points(a.points, true)
      case 'rect': {
        const [x, y, w, h] = [num(a.x), num(a.y), num(a.width), num(a.height)]
        const rx = a.rx === undefined ? 0 : Math.min(num(a.rx), w / 2)
        const ry = a.ry === undefined ? rx : Math.min(num(a.ry), h / 2)
        if (!rx && !ry) return `M${x} ${y}H${num(x + w)}V${num(y + h)}H${x}Z`
        const arc = (dx, dy) => `a${rx} ${ry} 0 0 1 ${num(dx)} ${num(dy)}`
        return `M${num(x + rx)} ${y}`
          + `H${num(x + w - rx)}${arc(rx, ry)}`
          + `V${num(y + h - ry)}${arc(-rx, ry)}`
          + `H${num(x + rx)}${arc(-rx, -ry)}`
          + `V${num(y + ry)}${arc(rx, -ry)}Z`
      }
      default:
        throw new Error(`unhandled element <${tag}>`)
    }
  }

  /**
   * The JavaScript half of `cide_ext::manifest::is_svg_path`.
   *
   * Restated here rather than imported, because the authority is Rust and this is a mirror: the
   * point of running it is to prove that every value this script emits is inside the same space
   * an extension's icon has to live in. `check-ui-icons.mjs` asserts the same thing on the
   * committed file, so a drift between the two is caught even if this script is never re-run.
   */
  const isSvgPath = (d) =>
    d.length > 0 && d.length <= 4096
    && /^[A-Za-z0-9.,\-+ \t\n\r]*$/.test(d)
    && [...d].filter((c) => /[A-Za-z]/.test(c)).every((c) => 'MmLlHhVvCcSsQqTtAaZz'.includes(c))

  const paths = new Map()
  const sources = new Map()
  for (const name of [...new Set([...ICONS, ...CSS_ICONS])].sort()) {
    const svg = readFileSync(join(CHECKOUT, 'icons', `${name}.svg`), 'utf8')
    const open = svg.slice(0, svg.indexOf('>'))
    for (const [attr, want] of ENVELOPE) {
      const got = new RegExp(`${attr}="([^"]*)"`).exec(open)?.[1]
      if (got !== want) {
        throw new Error(
          `${name}.svg: ${attr} is "${got}", expected "${want}". Upstream changed the envelope; `
            + 'every mark in this app inherits it, so this must be looked at rather than worked around.',
        )
      }
    }
    const tags = []
    let d = ''
    for (const m of svg.slice(svg.indexOf('>') + 1).matchAll(/<(\w+)\s([^>]*?)\/>/g)) {
      const attrs = Object.fromEntries(
        [...m[2].matchAll(/([\w-]+)="([^"]*)"/g)].map((a) => [a[1], a[2]]),
      )
      if ('fill' in attrs) {
        throw new Error(
          `${name}.svg has a filled <${m[1]}>. Flattening to a stroke-only path would silently `
            + 'lose that ink, so this icon needs a different treatment or a different icon.',
        )
      }
      tags.push(m[1])
      const part = toPath(m[1], attrs)
      if (!/^[MLHVCSQTAZ]/.test(part)) {
        throw new Error(
          `${name}: a subpath starts with "${part[0]}", which is relative. Concatenation is only `
            + 'correct when every subpath after the first opens absolutely — see `absolutise`.',
        )
      }
      d += part
    }
    if (!isSvgPath(d)) throw new Error(`${name}: flattened path is outside is_svg_path`)
    paths.set(name, d)
    sources.set(name, [...new Set(tags)])
  }

  // -------------------------------------------------------------------------------------
  // Emit.
  // -------------------------------------------------------------------------------------

  const rows = [...paths.entries()].map(([name, d]) => {
    const from = sources.get(name)
    // Only annotate the icons that went through the flattener; a plain <path> needs no note.
    const note = from.length === 1 && from[0] === 'path' ? '' : `  // from ${from.join(' + ')}`
    return `  '${name}': '${d}',${note}`
  })

  /*
   * The banner is a bang-prefixed legal comment, which esbuild and Rolldown *can* preserve through
   * minification — and this build does not. Checked rather than assumed: after `pnpm build` the
   * string "Lucide Icons and Contributors" appears only in `dist/assets/index-*.js.map`, never
   * in the JS itself. So the banner is documentation for whoever opens this file, and
   * `public/LUCIDE-LICENSE` is what actually ships. If the bundler's legal-comment handling ever
   * changes, that is a bonus, not the mechanism.
   */
  writeFileSync(OUT_TS, `\
/*! Lucide — ISC. Copyright (c) Lucide Icons and Contributors.
 *  Portions derived from Feather (MIT). Full text: ui/public/LUCIDE-LICENSE. */
/**
 * GENERATED — do not hand-edit. \`node ui/scripts/vendor-ui-icons.mjs\` writes this file.
 *
 * Transcribed from lucide-icons/lucide at ${UPSTREAM_REV} (1.33.0), ISC, by replaying upstream's
 * own artwork rather than redrawing it. Every value is a single 24x24 path \`d\`: multi-element
 * icons are flattened at vendor time, which is what keeps a built-in mark and an
 * extension-contributed one the same kind of value — see the script's header for why that
 * matters, and \`crates/cide-ext/src/manifest.rs::is_svg_path\` for the space they share.
 *
 * The set is closed: \`check-ui-icons.mjs\` fails on a name nothing draws and on a name drawn
 * that is not here. To add one, edit \`ICONS\` in the script and re-run it.
 *
 * The ISC notice reaches a shipped build through \`ui/public/LUCIDE-LICENSE\`, which \`public/\`
 * copies verbatim into \`dist\` — the same route the Material file-icon set's licence takes. The
 * \`/*!\` banner above is for whoever opens *this* file: it does not survive minification in this
 * build, which was checked rather than assumed.
 */
export const ICON_PATHS = {
${rows.join('\n')}
} as const

/** Every name \`<Icon>\` will draw. A literal union, so a typo is a compile error. */
export type IconName = keyof typeof ICON_PATHS
`)

  /*
   * The CSS half.
   *
   * The stroke is baked to black rather than left as `currentColor`, and that is not a loss: in
   * a `mask-image` the SVG is a stencil, so its own colour resolves against its own initial
   * value and never against the host element's. What reaches the screen is the *element's*
   * `background-color`, so `var(--red)` and `var(--yellow)` still work and both themes still
   * follow — which a `background-image` data URI could not have done.
   *
   * `img-src 'self' data:` in `tauri.conf.json` already covers these; a mask is fetched under
   * `img-src`. `mask-image` itself was one of the fourteen properties probed on this engine when
   * `check-css-prefix.mjs` was written, and it was not among the failures.
   */
  const uri = (d) =>
    `url("data:image/svg+xml,${encodeURIComponent(
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" `
      + `stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="${d}"/></svg>`,
    ).replace(/'/g, '%27').replace(/"/g, '%22')}")`

  writeFileSync(join(UI, 'src/styles/iconMasks.css'), `\
/*
 * GENERATED — do not hand-edit. \`node ui/scripts/vendor-ui-icons.mjs\` writes this file.
 *
 * The marks that CSS has to draw, because their site is a \`content:\` rule rather than a
 * component: the CodeMirror lint gutter and the markdown preview's external-link marker. Used
 * as \`mask-image\`, with the colour coming from the element's \`background-color\` — see the
 * script for why that is the only form that keeps a token colour.
 */
:root {
${CSS_ICONS.map((n) => `  --icon-${n}: ${uri(paths.get(n))};`).join('\n')}
}
`)

  copyFileSync(join(CHECKOUT, 'LICENSE'), OUT_LICENCE)
  console.log(`vendored ${paths.size} icons from lucide @ ${UPSTREAM_REV.slice(0, 8)}`)
  const flattened = [...sources.values()].filter((s) => s.length > 1 || s[0] !== 'path').length
  console.log(`  ${flattened} needed the flattener; ${paths.size - flattened} were a single <path>`)
} finally {
  rmSync(CHECKOUT, { recursive: true, force: true })
}
