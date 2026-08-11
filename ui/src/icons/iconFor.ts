/**
 * File-tree icons: name in, icon file *stem* out.
 *
 * The associations are the Material Icon Theme's, transcribed into `iconMap.ts` from
 * material-extensions/vscode-material-icon-theme (MIT — `ui/public/icons/LICENSE`). This file
 * is the resolution *rule* that turns a row into one of them, and it is deliberately the only
 * hand-written half: the point of the exercise is that a `.rs` looks like the icon people
 * already know, so an association invented here would be a bug even if it looked reasonable.
 *
 * Values in, values out — the same rule as `settings/theme.ts` and `sidebar/treeStatus.ts`.
 * The DOM, the URL and the theme subscription live in `FileIcon.tsx`; everything that can be
 * wrong on its own lives here, where `scripts/check-icons.mjs` compiles it with `tsc` and
 * asserts on it. That check also walks every stem this file can return against
 * `public/icons/`, because the failure mode of the `<img>` renderer is not an exception — it
 * is a row that silently draws nothing, which no screenshot review reliably catches.
 *
 * Its only import is the generated table, which imports nothing.
 */
import { BY_EXTENSION, BY_FILENAME, BY_FOLDER, HAS_LIGHT_VARIANT } from './iconMap'

/** Structurally the app's `Theme`; kept local so this file stays cheap to compile alone. */
export type IconTheme = 'dark' | 'light'

/**
 * What the lookup needs off a row.
 *
 * Written as a structural subset of the generated `TreeRow` on purpose, so `FileTree` can pass
 * the row it already has rather than destructure one — and so a field renamed in `cide-fs`
 * breaks at the call site instead of here. `expanded` is optional because a caller that only
 * has a name (a tab, a picker result, a diff header) should not have to invent one.
 */
export interface IconRow {
  /** The last path component. Matching is case-insensitive; the raw value is fine. */
  readonly name: string
  readonly kind: 'dir' | 'file'
  /** Directories only. Absent or false draws the closed folder. */
  readonly expanded?: boolean | undefined
}

/** What an unmatched file gets. Upstream's `fileIcons.defaultIcon`. */
export const DEFAULT_FILE_ICON = 'file'

/** What an unmatched directory gets, before the `-open` suffix. */
export const DEFAULT_FOLDER_ICON = 'folder'

const LIGHT = new Set(HAS_LIGHT_VARIANT)

/**
 * The icon file stem for a row — `'rust'`, `'folder-src-open'`, `'javascript_light'`.
 *
 * A stem, not a URL: this module has to compile and run under bare `tsc` in the check script,
 * and `import.meta.env.BASE_URL` is a Vite substitution that would drag the whole bundler in.
 * `iconUrl` in `FileIcon.tsx` owns that half.
 *
 * Every stem it can return has a file in `public/icons/`, and `check-icons.mjs` is what makes
 * that a fact rather than an intention.
 */
export function iconFor(row: IconRow, theme: IconTheme): string {
  const stem = row.kind === 'dir' ? folderStem(row) : fileStem(row.name)
  // `_light` is a second *file*, not a CSS filter: an `<img>` is opaque to the page's
  // stylesheet, and these icons are multi-colour by design, so there is no single hue to
  // override. Asking for one that does not exist is a 404 and a blank row, hence the set.
  return theme === 'light' && LIGHT.has(stem) ? `${stem}_light` : stem
}

/**
 * Upstream's file resolution, which VS Code's icon-theme contract fixes:
 *
 *   1. the whole file name wins over any extension — that is what makes `Cargo.lock` the lock
 *      and `package.json` the node icon rather than generic JSON;
 *   2. otherwise the LONGEST trailing extension wins, so `client.test.ts` is the test icon and
 *      not merely TypeScript;
 *   3. otherwise the generic file.
 *
 * Note what step 2 does with a dotfile: `.eslintrc` has no name entry of its own, so the loop
 * offers `eslintrc` as an extension, which is exactly how upstream's own table is keyed.
 */
function fileStem(name: string): string {
  const lower = name.toLowerCase()

  const byName = BY_FILENAME[lower]
  if (byName !== undefined) return byName

  // Successive suffixes, longest first: `a.b.c` offers `b.c` then `c`. Starting at 1 rather
  // than 0 is what keeps the whole name out of the extension table — that is step 1's job,
  // and letting it run here would make `tsconfig.json` reachable by two different rules.
  const parts = lower.split('.')
  for (let i = 1; i < parts.length; i++) {
    const hit = BY_EXTENSION[parts.slice(i).join('.')]
    if (hit !== undefined) return hit
  }

  return DEFAULT_FILE_ICON
}

/**
 * Upstream's folder resolution, plus the decoration-stripping its flat manifest does by
 * writing five keys instead.
 *
 * `extendFolderNames` emits `x`, `.x`, `_x`, `-x` and `__x__` for every name it knows, because
 * a VS Code icon theme is a lookup table with nowhere to put a rule. `BY_FOLDER` keeps the
 * canonical name only and the decoration comes off here, which is why `.github`, `_test` and
 * `__tests__` all land on their icon without an entry each.
 */
function folderStem(row: IconRow): string {
  const lower = row.name.toLowerCase()
  const stem = BY_FOLDER[lower] ?? BY_FOLDER[canonicalFolder(lower)] ?? DEFAULT_FOLDER_ICON
  return row.expanded === true ? `${stem}-open` : stem
}

/**
 * `.github` → `github`, `__tests__` → `tests`, `-lib` → `lib`.
 *
 * The undecorated form is tried *second*, never first: a folder actually named `.git` must be
 * allowed to match a `.git` entry if upstream ever adds one, rather than being rewritten into
 * `git` behind its back.
 */
function canonicalFolder(lower: string): string {
  const wrapped = /^__(.+)__$/.exec(lower)
  if (wrapped?.[1] !== undefined) return wrapped[1]
  // One separator, not a greedy strip: `--x` is a folder called `-x`, not one called `x`.
  return /^[._-]/.test(lower) ? lower.slice(1) : lower
}
