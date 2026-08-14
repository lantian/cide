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
