/**
 * Which language a path is, and how to load it.
 *
 * The load is a dynamic `import()` per language, which is the whole point: Vite emits one
 * chunk per module below, and opening a `.toml` never downloads or parses the Rust
 * keyword table. The extension table itself is static and tiny, so a tab strip can label a
 * file without loading anything.
 *
 * `languageName` is separated from `loadLanguage` for the same reason. The status bar
 * has to say `Rust` the moment the buffer appears — before the chunk has resolved, and
 * still for a file too large to highlight at all.
 */
import { StreamLanguage } from '@codemirror/language'
import type { Extension } from '@codemirror/state'

import type { FoldSpec } from './foldRanges'
import type { Grammar } from './streamGrammar'

/** A module identifier in this registry, not a user-visible name. */
export type LanguageId =
  | 'rust'
  | 'go'
  | 'typescript'
  | 'python'
  | 'clike'
  | 'shell'
  | 'json'
  | 'toml'
  | 'yaml'
  | 'markdown'
  | 'sql'

interface Entry {
  /** What the status bar readout calls it. */
  label: string
  /**
   * The built tokenizer, behind the dynamic `import()`.
   *
   * The registry holds the *grammar* and [`loadLanguage`] wraps it, rather than each entry
   * building its own `StreamLanguage`. Two callers need the two different things: the buffer
   * wants a CodeMirror extension, and `editor/markdown/fenceTokens.ts` wants the parser itself,
   * because it colours a fenced code block with no `EditorView` anywhere near it and
   * `StreamLanguage` does not expose the parser it was defined from.
   */
  grammar: () => Promise<Grammar>
}

const REGISTRY: Record<LanguageId, Entry> = {
  rust: {
    label: 'Rust',
    grammar: async () => (await import('./languages/rust')).spec,
  },
  go: {
    label: 'Go',
    grammar: async () => (await import('./languages/go')).spec,
  },
  typescript: {
    label: 'TypeScript',
    grammar: async () => (await import('./languages/typescript')).spec,
  },
  python: {
    label: 'Python',
    grammar: async () => (await import('./languages/python')).spec,
  },
  clike: {
    label: 'C-like',
    grammar: async () => (await import('./languages/clike')).spec,
  },
  shell: {
    label: 'Shell',
    grammar: async () => (await import('./languages/shell')).spec,
  },
  json: {
    label: 'JSON',
    grammar: async () => (await import('./languages/data')).json,
  },
  toml: {
    label: 'TOML',
    grammar: async () => (await import('./languages/data')).toml,
  },
  yaml: {
    label: 'YAML',
    grammar: async () => (await import('./languages/data')).yaml,
  },
  markdown: {
    label: 'Markdown',
    grammar: async () => (await import('./languages/markdown')).spec,
  },
  sql: {
    label: 'SQL',
    grammar: async () => (await import('./languages/sql')).spec,
  },
}

/**
 * Extension → language. Lowercase, without the dot.
 *
 * Some entries are labelled by their extension rather than by their module: a `.tsx` file
 * is `TSX` in the readout even though it loads the TypeScript table, because that is what
 * the tab badge says and the two disagreeing would read as a bug.
 */
const BY_EXTENSION: Record<string, { id: LanguageId; label?: string }> = {
  rs: { id: 'rust' },
  ts: { id: 'typescript' },
  mts: { id: 'typescript' },
  cts: { id: 'typescript' },
  tsx: { id: 'typescript', label: 'TSX' },
  js: { id: 'typescript', label: 'JavaScript' },
  mjs: { id: 'typescript', label: 'JavaScript' },
  cjs: { id: 'typescript', label: 'JavaScript' },
  jsx: { id: 'typescript', label: 'JSX' },
  py: { id: 'python' },
  pyi: { id: 'python' },
  c: { id: 'clike', label: 'C' },
  h: { id: 'clike', label: 'C' },
  cc: { id: 'clike', label: 'C++' },
  cpp: { id: 'clike', label: 'C++' },
  cxx: { id: 'clike', label: 'C++' },
  hpp: { id: 'clike', label: 'C++' },
  go: { id: 'go' },
  java: { id: 'clike', label: 'Java' },
  sh: { id: 'shell' },
  bash: { id: 'shell' },
  zsh: { id: 'shell' },
  fish: { id: 'shell' },
  json: { id: 'json' },
  jsonc: { id: 'json' },
  toml: { id: 'toml' },
  lock: { id: 'toml', label: 'TOML' },
  yaml: { id: 'yaml' },
  yml: { id: 'yaml' },
  md: { id: 'markdown' },
  markdown: { id: 'markdown' },
  sql: { id: 'sql' },
}

/**
 * What the fold scanner needs to know about each language.
 *
 * # Why this is a second table and not a field of the grammar
 *
 * It is *nearly* a field of the grammar — six of its keys mirror `GrammarSpec` exactly and must
 * never disagree with it. But a grammar arrives through a dynamic `import()`, and fold restore
 * has to run inside the editor's **mount dispatch**: the remembered scroll position is a line
 * number, and applying it against unfolded heights and then folding a tick later lands the user
 * somewhere they did not leave. Waiting for the chunk would cost that; so this is static, and it
 * costs about a line of source per language.
 *
 * Drift is made a build failure rather than trusted. `grammar()` in `streamGrammar.ts` hands its
 * input back as `.spec`, and `ui/scripts/check-editor.mjs` asserts every mirrored field of every
 * entry below equals the grammar's. A fold spec that thought `#` opened a comment in Rust would
 * treat every `#[derive(…)]` as a dead line, and nothing on screen would say so.
 *
 * # What each language actually gets
 *
 * `regions` is on for everything with a line comment — an explicit `// region` marker is the one
 * fold a person writes down on purpose, and refusing to honour it in six of eleven languages
 * would be arbitrary. `indentBlocks` is on only where indentation is *semantic* (Python, YAML);
 * turning it on for a brace language would offer a second, worse fold on every line that already
 * has a good one.
 */
const FOLD: Readonly<Record<LanguageId, FoldSpec>> = {
  rust: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
    nestedComments: true,
    // A `"` genuinely spans lines in Rust, and `rawStrings` is here rather than anywhere else
    // because `r#"…"#` is where an unbalanced brace most often lives — a `format!` template, an
    // embedded shell script, a regex.
    multilineQuotes: '"',
    rawStrings: true,
    // `fn f<'a>(x: &'a str) {` — without this the `'` opens a string, the trailing `{` is inside
    // it, and the function is not foldable. In a codebase with lifetimes that is most of it.
    lifetimes: true,
    regions: true,
  },
  go: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
    nestedComments: false,
    quotes: '"',
    extraQuotes: '`',
    multilineQuotes: '`',
    regions: true,
  },
  typescript: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
    extraQuotes: '`',
    multilineQuotes: '`',
    regions: true,
  },
  python: {
    lineComment: '#',
    tripleQuotes: true,
    indentBlocks: true,
    regions: true,
  },
  clike: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
    regions: true,
  },
  shell: {
    lineComment: '#',
    // Both quotes span lines in every shell, which is not true of most languages and is why the
    // scanner's default is the other way round.
    multilineQuotes: '"\'',
    regions: true,
  },
  json: {
    lineComment: '//',
    blockComment: ['/*', '*/'],
    quotes: '"',
  },
  toml: {
    lineComment: '#',
    quotes: '"\'',
    tripleQuotes: true,
    regions: true,
  },
  yaml: {
    lineComment: '#',
    quotes: '"\'',
    indentBlocks: true,
    regions: true,
  },
  markdown: {
    // No strings and no brackets: a `(` in prose is not a block, and `quotes: ''` is what the
    // grammar says too — an apostrophe in *don't* would otherwise open a string.
    quotes: '',
    brackets: '',
    headingFolds: true,
    fencedBlocks: true,
  },
  sql: {
    lineComment: '--',
    blockComment: ['/*', '*/'],
    nestedComments: false,
    quotes: '\'"`',
    escapes: false,
    multilineQuotes: "'",
    regions: true,
  },
}

/**
 * What a buffer with no known language folds by.
 *
 * Brackets and indentation, and **no string or comment awareness at all** — guessing that `"`
 * opens a string in a file that might be prose would let one apostrophe hide every brace on its
 * line. A `.txt` file with an indented block still folds, which is more than IDEA offers.
 *
 * It is also why this is a value rather than a `null`: every buffer gets a fold gutter, so the
 * gutter reservation in `EditorSurface.module.css` is one constant width instead of a layout that
 * shifts when you switch from a `.rs` tab to a `.txt` one.
 */
export const DEFAULT_FOLD_SPEC: FoldSpec = { quotes: '', indentBlocks: true }

/** How to fold this path. Never null — see [`DEFAULT_FOLD_SPEC`]. */
export function foldSpecFor(path: string): FoldSpec {
  const found = lookup(path)
  return found === null ? DEFAULT_FOLD_SPEC : FOLD[found.id]
}

/** Testing seam: the fold table, for the check that pins it against the grammars. */
export function foldSpecs(): Readonly<Record<string, FoldSpec>> {
  return FOLD
}

/**
 * A type the *New scratch file…* picker offers.
 *
 * The `ext` is what crosses the wire — `fs.scratchNew(project, 'rs')` — and what names the file
 * on disk, so it is also what decides the language of the buffer that opens.
 */
export interface ScratchType {
  /** The row's label, and what the status bar will say once the file is open. */
  readonly label: string
  /** The extension, without the dot. Lowercase letters and digits; Rust refuses anything else. */
  readonly ext: string
}

/**
 * The types a scratch file may be, in the order the picker lists them.
 *
 * # Why this lives here and not in the overlay
 *
 * Because this file is the thing it must not disagree with. [`lookup`] resolves an extension to
 * a grammar and a label; if the picker carried its own array of `{label, ext}` pairs, the first
 * entry whose extension is not in [`BY_EXTENSION`] would produce a scratch offered as *YAML*
 * that opens with no highlighting and a status bar reading `Plain Text` — a failure that is
 * silent, specific, and invisible to `tsc`. Keeping the two in one file does not prevent that
 * on its own, so `ui/scripts/check-editor.mjs` asserts that **every entry's extension resolves
 * through `lookup` to that entry's own label**, which makes the disagreement unrepresentable.
 *
 * # What is offered, and what is not
 *
 * One row per language in [`REGISTRY`] that a person would name, ordered by what this
 * application is for, plus plain text. `.txt` is the deliberate exception to the rule above:
 * it is **not** in `BY_EXTENSION` and must not be added — that would be a language with no
 * grammar — so it resolves to `Plain Text` by the same fallback every unknown extension takes,
 * and the check special-cases exactly that.
 *
 * `C-like` is not offered under that name: the module is reached under six different labels
 * (`c`, `cpp`, `java`, …) and "C-like" is an implementation detail, not something a person
 * picks. C and C++ are listed separately and point at the same loader, which is precisely what
 * the per-extension `label` override in `BY_EXTENSION` exists for.
 */
export const SCRATCH_TYPES: readonly ScratchType[] = [
  { label: 'Rust', ext: 'rs' },
  { label: 'Go', ext: 'go' },
  { label: 'TypeScript', ext: 'ts' },
  { label: 'JavaScript', ext: 'js' },
  { label: 'Python', ext: 'py' },
  { label: 'JSON', ext: 'json' },
  { label: 'YAML', ext: 'yaml' },
  { label: 'TOML', ext: 'toml' },
  { label: 'Markdown', ext: 'md' },
  { label: 'Shell', ext: 'sh' },
  // After Shell and before C, which is where a person looking for it would scan. Added in M15
  // with its grammar (`languages/sql.ts`) rather than as a bare row: `check-editor.mjs` requires
  // every offered extension to resolve through `lookup` to that entry's own label *and* to load
  // a real grammar, so a type the editor cannot highlight is a build failure rather than a
  // scratch that quietly opens as plain text.
  { label: 'SQL', ext: 'sql' },
  { label: 'C', ext: 'c' },
  { label: 'C++', ext: 'cpp' },
  { label: 'Plain Text', ext: 'txt' },
]

/**
 * The rows [`SCRATCH_TYPES`] offers for a typed query, best first.
 *
 * # Why the picker gained a filter at all
 *
 * `ScratchType.tsx` used to argue, in its header, that "there is nothing worth fuzzy-matching
 * in `Rust`, `Go`, `JSON`". That was true of thirteen rows read as a list and false of the
 * gesture: reaching SQL from the top of the list is five Down presses, while `sql` is three
 * characters and lands on it. This reverses that decision, and the header there is rewritten
 * rather than left standing.
 *
 * # Ranked, not plain-substring
 *
 * A bare `label.includes(q)` answers `js` with **JSON** — `J`… no, with nothing, and `s` with
 * *TypeScript, JavaScript, JSON, Shell, SQL* in list order, so the top row for `s` is whichever
 * happens to be highest rather than the one whose name starts that way. So: five tiers, exact
 * extension first, and list order inside a tier.
 *
 *   1. the extension exactly — `sql` is SQL, `md` is Markdown, `c` is C and not C++
 *   2. the extension by prefix — `ja` reaches nothing, `j` reaches JavaScript and JSON
 *   3. the label by prefix — `ru` is Rust, `ty` is TypeScript
 *   4. the label by substring — `script` reaches TypeScript and JavaScript
 *   5. the extension by substring — the long tail
 *
 * Case-insensitive throughout, because the labels are capitalised and the extensions are not,
 * and nobody types `SQL` looking for `.sql`.
 *
 * # Why here and not in the overlay
 *
 * The same argument [`SCRATCH_TYPES`] makes above: this file is the thing the picker must not
 * disagree with, and a filter beside the list it filters cannot drift from it. It is also what
 * makes the rule *drivable* — `check-editor.mjs` already requires the compiled `languages.js`,
 * so this needs no new plumbing to be tested, where a predicate inside the component would have
 * needed a DOM.
 *
 * An empty query is every row, in list order — which is what makes the popup's initial state
 * the same list it has always shown.
 */
export function filterScratchTypes(query: string): readonly ScratchType[] {
  const q = query.trim().toLowerCase()
  if (q === '') return SCRATCH_TYPES
  const tier = (type: ScratchType): number => {
    const ext = type.ext.toLowerCase()
    const label = type.label.toLowerCase()
    if (ext === q) return 0
    if (ext.startsWith(q)) return 1
    if (label.startsWith(q)) return 2
    if (label.includes(q)) return 3
    if (ext.includes(q)) return 4
    return 5
  }
  return SCRATCH_TYPES.map((type, index) => ({ type, index, tier: tier(type) }))
    .filter((row) => row.tier < 5)
    // Ties by list order, so the ordering is total and the top row for a given query never
    // depends on `Array.prototype.sort`'s stability guarantees for a comparator that returned 0.
    .sort((a, b) => a.tier - b.tier || a.index - b.index)
    .map((row) => row.type)
}

/**
 * Which row the type picker opens on, given the file the user is looking at.
 *
 * The language of the focused editor's file when it is one of the offered types, else the first
 * row. IDEA preselects nothing and makes every scratch a two-decision gesture; preselecting
 * makes the common case — *another one of these* — two keystrokes, and costs nothing when the
 * guess is wrong because the list is right there.
 *
 * Matched through [`lookup`] rather than on the extension string, in two passes, because the
 * extension the user is looking at is usually **not** one of the offered ones:
 *
 *   1. the *label* — so `.mjs` preselects `JavaScript` rather than `TypeScript`, even though
 *      both load the same module;
 *   2. failing that, the *grammar* — so `.tsx` preselects `TypeScript` and `.java` preselects
 *      `C`, which is the offered row that loads the same table. A one-row-off guess on a
 *      language nobody offers beats falling back to Rust because the list happens to start
 *      there.
 *
 * A file whose extension resolves to nothing preselects the row that also resolves to nothing,
 * which is `Plain Text` — the same fallback, arrived at the same way, rather than a second rule
 * naming `'txt'`.
 *
 * Pure and index-returning rather than a `ScratchType`, because the overlay's selection state is
 * an index: handing it a value it would then have to `indexOf` is two representations of one
 * fact. Never out of range — [`SCRATCH_TYPES`] is non-empty and `0` is the fallback.
 */
export function defaultScratchType(path: string | null): number {
  if (path === null) return 0
  const found = lookup(path)
  const at = SCRATCH_TYPES.findIndex((type) => {
    const offered = lookup(`scratch.${type.ext}`)
    if (found === null) return offered === null
    return offered !== null && offered.label === found.label
  })
  if (at >= 0) return at
  if (found === null) return 0
  const sameGrammar = SCRATCH_TYPES.findIndex(
    (type) => lookup(`scratch.${type.ext}`)?.id === found.id,
  )
  return sameGrammar < 0 ? 0 : sameGrammar
}

/**
 * Files with no extension that are still a known language.
 *
 * Keyed on the whole file name, and lowercased before lookup — `Dockerfile` and
 * `dockerfile` are the same file to everyone but the string comparison.
 */
const BY_NAME: Record<string, LanguageId> = {
  '.bashrc': 'shell',
  '.zshrc': 'shell',
  '.profile': 'shell',
  '.gitconfig': 'toml',
  dockerfile: 'shell',
  makefile: 'shell',
}

export function basename(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return path.slice(cut + 1)
}

function lookup(path: string): { id: LanguageId; label: string } | null {
  const name = basename(path).toLowerCase()
  const byName = BY_NAME[name]
  if (byName) return { id: byName, label: REGISTRY[byName].label }

  const dot = name.lastIndexOf('.')
  // `> 0` rather than `>= 0`: a leading dot makes a hidden file, not an extension, so
  // `.gitignore` is not looked up as a `gitignore` language.
  const ext = dot > 0 ? name.slice(dot + 1) : ''
  const found = BY_EXTENSION[ext]
  if (!found) return null
  return { id: found.id, label: found.label ?? REGISTRY[found.id].label }
}

/** What the status bar calls this file's language. `Plain Text` when nothing matches. */
export function languageName(path: string): string {
  return lookup(path)?.label ?? 'Plain Text'
}

/**
 * Which grammar a path resolves to, or null. (M20)
 *
 * The *id*, where [`languageName`] gives the label, and the difference matters to any caller
 * asking a question about the language rather than displaying it. Six extensions share the
 * `clike` grammar under six different labels, and `md` and `markdown` share one under one — so
 * `languageName(path) === 'Markdown'` happens to work today and is a string comparison against a
 * user-facing caption, which is the kind of test that stops being true the day somebody
 * capitalises it differently.
 *
 * Added for `editor/markdown/`, which has to know whether a pane may be previewed at all.
 */
export function languageIdFor(path: string): LanguageId | null {
  return lookup(path)?.id ?? null
}

/** Is this a file the markdown preview can render? (M20) */
export function isMarkdownPath(path: string): boolean {
  return languageIdFor(path) === 'markdown'
}

/**
 * The highlighting extension for a path, or null if there is none.
 *
 * Resolves to null rather than rejecting when the chunk fails to load: a network hiccup
 * behind the dev server, or a corrupted asset in a packaged build, should cost colour and
 * nothing else. An uncaught rejection here would leave the buffer permanently blank.
 */
export async function loadLanguage(path: string): Promise<Extension | null> {
  const found = lookup(path)
  if (!found) return null
  const spec = await loadGrammar(found.id)
  return spec === null ? null : StreamLanguage.define(spec)
}

/**
 * The tokenizer for a language id, or null if its chunk will not load. (M20)
 *
 * The half of [`loadLanguage`] that is not about CodeMirror. Both swallow a failed import for
 * the same reason: a network hiccup behind the dev server, or a corrupted asset in a packaged
 * build, should cost colour and nothing else, and an uncaught rejection out of a dynamic import
 * inside a React render leaves the surface permanently blank.
 */
export async function loadGrammar(id: LanguageId): Promise<Grammar | null> {
  try {
    return await REGISTRY[id].grammar()
  } catch (error) {
    console.error(`[cide] could not load the ${REGISTRY[id].label} grammar`, error)
    return null
  }
}
