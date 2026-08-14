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

/** A module identifier in this registry, not a user-visible name. */
type LanguageId =
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

interface Entry {
  /** What the status bar readout calls it. */
  label: string
  load: () => Promise<Extension>
}

const REGISTRY: Record<LanguageId, Entry> = {
  rust: {
    label: 'Rust',
    load: async () => StreamLanguage.define((await import('./languages/rust')).spec),
  },
  go: {
    label: 'Go',
    load: async () => StreamLanguage.define((await import('./languages/go')).spec),
  },
  typescript: {
    label: 'TypeScript',
    load: async () => StreamLanguage.define((await import('./languages/typescript')).spec),
  },
  python: {
    label: 'Python',
    load: async () => StreamLanguage.define((await import('./languages/python')).spec),
  },
  clike: {
    label: 'C-like',
    load: async () => StreamLanguage.define((await import('./languages/clike')).spec),
  },
  shell: {
    label: 'Shell',
    load: async () => StreamLanguage.define((await import('./languages/shell')).spec),
  },
  json: {
    label: 'JSON',
    load: async () => StreamLanguage.define((await import('./languages/data')).json),
  },
  toml: {
    label: 'TOML',
    load: async () => StreamLanguage.define((await import('./languages/data')).toml),
  },
  yaml: {
    label: 'YAML',
    load: async () => StreamLanguage.define((await import('./languages/data')).yaml),
  },
  markdown: {
    label: 'Markdown',
    load: async () => StreamLanguage.define((await import('./languages/markdown')).spec),
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
  { label: 'C', ext: 'c' },
  { label: 'C++', ext: 'cpp' },
  { label: 'Plain Text', ext: 'txt' },
]

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
 * The highlighting extension for a path, or null if there is none.
 *
 * Resolves to null rather than rejecting when the chunk fails to load: a network hiccup
 * behind the dev server, or a corrupted asset in a packaged build, should cost colour and
 * nothing else. An uncaught rejection here would leave the buffer permanently blank.
 */
export async function loadLanguage(path: string): Promise<Extension | null> {
  const found = lookup(path)
  if (!found) return null
  try {
    return await REGISTRY[found.id].load()
  } catch (error) {
    console.error(`[cide] could not load the ${found.label} grammar`, error)
    return null
  }
}
