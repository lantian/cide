/**
 * The fence around Settings' copy. (M133)
 *
 * # Why this exists
 *
 * The Settings screen was reported as hard to read and hard to understand: a lot of text on
 * screen, and blue and amber notes sitting close to controls without saying which one they were
 * about. The redesign moved the long explanations behind an (i) per row (`info`), tied warnings
 * to the row they concern (`status`), and added search over every row. All three decay the same
 * way — silently, one reasonable-looking addition at a time: a hint that "just needs one more
 * sentence", a `Note` dropped between two rows because it is the quickest way to say something,
 * a new row nobody added to the search index, which then is the one row search cannot find.
 *
 * # What it asserts
 *
 *   1. **Every literal `hint="…"` is at most `HINT_MAX` characters.** One line under a label at
 *      the 620px body width. Anything longer goes in `info`.
 *   2. **No settings file imports `Note` from `./controls`**, and the wrapper is gone from it —
 *      and the kit's own `Note` is imported only by the files in `NOTE_ALLOWED`, each of which
 *      says in a comment why its note cannot be a row's `status` or `info`.
 *   3. **Every literal `<Row label="…">` / `<ToggleRow label="…">` has an entry in
 *      `settingsIndex.ts`** (its `anchor="…"` when it has one), because search scrolls to
 *      `[data-setting="<label>"]` and an unlisted row is unfindable.
 *
 * Comments are stripped first (`CLAUDE.md`: house-style comments name the failure, so the needle
 * is in the prose).
 *
 * Run: `pnpm --dir ui run check:settings-copy`
 */
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

const SRC = resolve('src')
let failed = 0
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

const HINT_MAX = 90

/** Files that may still draw a kit `Note`, and must say why beside it. */
const NOTE_ALLOWED = new Set([
  // Conflicts and problems in the user's keymap file: about the file and several bindings at
  // once, drawn above the table rather than between rows.
  'settings/KeymapSection.tsx',
  // The role dialog's footer ("Nothing was written", "could not be opened"): the outcome of the
  // dialog's Save, not of any one field.
  'settings/AgentsSection.tsx',
])

function stripComments(text) {
  let out = ''
  let i = 0
  while (i < text.length) {
    const two = text.slice(i, i + 2)
    if (two === '/*') {
      const end = text.indexOf('*/', i + 2)
      const stop = end === -1 ? text.length : end + 2
      out += text.slice(i, stop).replace(/[^\n]/g, ' ')
      i = stop
    } else if (two === '//' && text[i - 1] !== ':') {
      // `text[i - 1] !== ':'` keeps a URL in a string (`https://…`) from eating its line.
      const end = text.indexOf('\n', i)
      const stop = end === -1 ? text.length : end
      out += ' '.repeat(stop - i)
      i = stop
    } else {
      out += text[i]
      i += 1
    }
  }
  return out
}

const settingsDir = join(SRC, 'settings')
const files = [
  ...readdirSync(settingsDir)
    .filter((n) => n.endsWith('.tsx'))
    .map((n) => join(settingsDir, n)),
  join(SRC, 'gitlab', 'GitLabSettings.tsx'),
]
const source = Object.fromEntries(
  files.map((f) => [relative(SRC, f), stripComments(readFileSync(f, 'utf8'))]),
)

// --- 1: one-line hints ---------------------------------------------------------------------

let hints = 0
for (const [file, text] of Object.entries(source)) {
  for (const m of text.matchAll(/\bhint="([^"]*)"/g)) {
    hints++
    ok(
      m[1].length <= HINT_MAX,
      `${file}: hint is ${m[1].length} characters (max ${HINT_MAX}) — move the rest into info: "${m[1].slice(0, 60)}…"`,
    )
  }
}
ok(hints > 30, `the settings screen has literal hints to measure (found ${hints})`)

// --- 2: no notes between rows --------------------------------------------------------------

const controls = source['settings/controls.tsx']
ok(!/export function Note\b/.test(controls), 'controls.tsx no longer exports the Note wrapper')
for (const [file, text] of Object.entries(source)) {
  ok(
    !/import\s*\{[^}]*\bNote\b[^}]*\}\s*from\s*'\.\/controls'/.test(text),
    `${file} imports Note from ./controls — a row's explanation is its info, a row's problem its status`,
  )
  const kitNote = /import\s*\{[^}]*\bNote\b[^}]*\}\s*from\s*'@\/kit\/components\/Feedback'/.test(text)
  ok(
    !kitNote || NOTE_ALLOWED.has(file),
    `${file} draws a kit Note — use a row's status/info or the page Banner, or add it to NOTE_ALLOWED with a reason`,
  )
}

// --- 3: every row is findable --------------------------------------------------------------

const index = stripComments(readFileSync(join(settingsDir, 'settingsIndex.ts'), 'utf8'))
const indexed = new Set([...index.matchAll(/\blabel:\s*(['"])(.*?)\1/g)].map((m) => m[2]))
let rows = 0
for (const [file, text] of Object.entries(source)) {
  for (const m of text.matchAll(/<(Row|ToggleRow)\s+label="([^"]+)"/g)) {
    // The row's own attributes run to its `control=` (a Row) or its first `/>` (a ToggleRow);
    // an `anchor="…"` in there is the key search uses instead of the label.
    const tail = text.slice(m.index, m.index + 1500)
    const end = Math.min(
      ...[tail.indexOf('control='), tail.indexOf('/>')].filter((n) => n >= 0),
      tail.length,
    )
    const anchor = /\banchor="([^"]+)"/.exec(tail.slice(0, end))?.[1]
    const key = anchor ?? m[2]
    rows++
    ok(indexed.has(key), `${file}: row "${key}" has no entry in settings/settingsIndex.ts — search cannot find it`)
  }
}
ok(rows > 40, `the settings screen has literal rows to index (found ${rows})`)

if (failed > 0) {
  console.error(`\n${failed} settings copy check(s) failed`)
  process.exit(1)
}
console.log(`settings copy: ok (${hints} hints ≤ ${HINT_MAX} chars, ${rows} rows indexed)`)
