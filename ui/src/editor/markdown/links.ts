/**
 * What a link or an image in a preview points at. (M20)
 *
 * Three kinds, and the whole feature's behaviour follows from which one a target is:
 *
 * * **External** — `https://…`, `mailto:`, anything with a scheme. cide does not navigate to
 *   these, and the refusal is the same one `terminal/xterm.ts` makes for OSC 8 hyperlinks, whose
 *   comment is worth reading: opening a URL "would have to go through Rust and
 *   `tauri_plugin_opener`, because the JS opener command is capability-gated per window and a
 *   detached-pane window deliberately has no `opener` permission, so a JS-side open would work in
 *   the shell window and silently do nothing in a torn-out pane". A `.md` in a cloned repository
 *   is exactly as untrusted as terminal output.
 * * **A fragment** — `#some-heading`, which scrolls this preview and touches nothing else.
 * * **Local** — a relative or absolute path, resolved against the document's own directory and
 *   opened as a tab, which is what a reader clicking `[the ADR](../adr/0009.md)` wants.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-markdown.mjs` compiles it standalone. Path resolution is arithmetic on
 * strings with a `..` in it, which is the shape of rule that is obviously right while you write
 * it and wrong for `a/../../b`, and there is no way to see that on screen.
 */

/** What kind of thing a link target is. */
export type Target = 'external' | 'fragment' | 'local'

/**
 * Anything with a `scheme:` prefix is external, and so is a protocol-relative `//host/path`.
 *
 * Deliberately **not** a list of allowed schemes. `javascript:` and `data:` must never be
 * followed, and the way to guarantee that is for nothing in this app to follow a scheme at all —
 * see [`Target`]. A Windows drive letter (`C:\…`) matches the scheme shape too and is treated as
 * external, which on this app's only platform is a path that could not be opened anyway.
 */
const SCHEME = /^[a-zA-Z][a-zA-Z0-9+.-]*:/

export function targetKind(href: string): Target {
  const trimmed = href.trim()
  if (trimmed.startsWith('#')) return 'fragment'
  if (trimmed.startsWith('//') || SCHEME.test(trimmed)) return 'external'
  return 'local'
}

/** The directory holding `path`, without a trailing slash. `''` for a bare filename. */
export function dirOf(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut <= 0 ? (cut === 0 ? '/' : '') : path.slice(0, cut)
}

/**
 * Resolve `target` against `dir`, collapsing `.` and `..`.
 *
 * Returns `null` when the target is not a local path, is empty, or climbs above the root — the
 * last of which is the case worth naming: `../../../../../../etc/passwd` from a document three
 * directories deep resolves to `/etc/passwd` on a naive implementation, and a preview that
 * happily *asks* for that path is a preview that has to be trusted not to. Refusing here is
 * belt; the brace is that `image_read` sniffs the header in Rust before granting the asset scope
 * anything, so a text file cannot be fetched however it was spelled.
 *
 * The query string and the fragment are stripped: `./img.png?v=2#x` is a file called `img.png`.
 */
export function resolveLocal(dir: string, target: string): string | null {
  if (targetKind(target) !== 'local') return null

  const cleaned = target.trim().split('#')[0]?.split('?')[0] ?? ''
  if (cleaned === '') return null

  const absolute = cleaned.startsWith('/')
  const base = absolute ? [] : dir.split('/').filter((part) => part !== '')
  const parts = [...base]

  for (const part of cleaned.split('/')) {
    if (part === '' || part === '.') continue
    if (part === '..') {
      if (parts.length === 0) return null
      parts.pop()
      continue
    }
    parts.push(part)
  }

  if (parts.length === 0) return null
  return `/${parts.join('/')}`
}

/**
 * The `id` a heading gets, so `[jump](#the-state-loop)` lands on it.
 *
 * GitHub's rule, near enough to be predictable from a document written for GitHub: lowercase,
 * drop everything that is not a letter, digit, space or hyphen, and turn runs of spaces into
 * single hyphens. Duplicates are made unique by the caller, which is the only part that needs to
 * see the whole document.
 */
export function slugify(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N} \t-]/gu, '')
    .replace(/[ \t]+/g, '-')
}

/**
 * Make a slug unique within a document, GitHub's way: `-1`, `-2`, …
 *
 * `seen` is mutated, so one map serves a whole render pass. Two headings with the same words is
 * ordinary in a document with repeated sections, and without this every one of them would take
 * the same anchor and every link to any of them would land on the first.
 */
export function uniqueSlug(text: string, seen: Map<string, number>): string {
  const base = slugify(text) === '' ? 'section' : slugify(text)
  const used = seen.get(base) ?? 0
  seen.set(base, used + 1)
  return used === 0 ? base : `${base}-${used}`
}

/** Extensions the image pipeline will accept, mirroring `panes/imageKinds.ts`. */
const IMAGE_EXT = /\.(png|jpe?g|gif|webp|bmp|ico|svg)$/i

/**
 * Is this local target something the asset protocol can serve as an image?
 *
 * Checked before `image.read` rather than after, so a `![](notes.md)` — which happens, because
 * `!` is a character people put in front of links by accident — never reaches Rust at all. It is
 * not a security boundary: `image_read` sniffs the header and refuses on the bytes, which is the
 * check that actually counts, and README's *Images* section explains why it is the one that does.
 */
export function looksLikeImage(path: string): boolean {
  return IMAGE_EXT.test(path)
}
