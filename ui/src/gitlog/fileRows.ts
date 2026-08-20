/**
 * The details pane's changed-file list, flat or grouped into directories.
 *
 * Import-free, and compiled standalone by `ui/scripts/check-log.mjs`. Tree building is the kind
 * of code that is obviously right and quietly wrong — the cases that break it are a file at the
 * root, two files whose directories share a prefix but not a segment (`src/app` and `src/apple`),
 * and a rename whose two halves live in different directories. None of those are reachable from a
 * component test.
 *
 * # What a group is
 *
 * The **longest common directory prefix is not collapsed away**, deliberately. IDEA collapses a
 * chain of single-child directories into `a/b/c` and this does too, because the alternative is
 * three rows of nothing but indentation. What it does not do is hoist the whole list under one
 * root: a commit touching only `crates/cide-git/src/` would otherwise draw one directory row and
 * then a flat list, which is the flat listing with an extra line above it.
 */

/** One entry the pane was given. Structural, not the generated DTO — see the header. */
export interface ChangedFile {
  readonly path: string
  /** The pre-rename path, or `null`. */
  readonly oldPath: string | null
  /** `+12 −3`, or `null` for a list that states its totals once. */
  readonly counts: string | null
}

/** A row to draw: a directory heading, or a file. */
export type FileRow =
  | {
      readonly kind: 'dir'
      readonly id: string
      readonly label: string
      readonly depth: number
      /** The directory this heading is for, which is what a collapse toggle names. */
      readonly dir: string
      readonly collapsed: boolean
      /** How many files are under it — drawn when collapsed, so a folded row is not silent. */
      readonly count: number
    }
  | {
      readonly kind: 'file'
      readonly id: string
      /** What the row shows — the basename when grouped, the whole path when flat. */
      readonly label: string
      readonly depth: number
      readonly file: ChangedFile
    }

/** The directory part of a path, or `''` for a file at the root. */
function dirOf(path: string): string {
  const at = path.lastIndexOf('/')
  return at === -1 ? '' : path.slice(0, at)
}

/** The last segment. `''` never happens for a real path, but a trailing slash would produce it. */
function baseOf(path: string): string {
  const at = path.lastIndexOf('/')
  const name = at === -1 ? path : path.slice(at + 1)
  return name === '' ? path : name
}

/**
 * Pixels of indentation per depth level.
 *
 * **19, which is the house number** — `sidebar/FileTree.tsx`'s `INDENT` and
 * `GitPanel/ChangesTree.tsx` both use it, and it is measured rather than chosen: in the IDEA
 * reference the same closed-folder glyph at depths 0/1/2 has its ink left edge at x = 73, 92,
 * 111. That file's note is worth repeating because this list got it wrong in exactly the way it
 * warns about — *"at 12px a nested tree reads as a flat list"*.
 *
 * The first attempt here used 21px of `padding-left` and produced no visible indent at all, for
 * a reason arithmetic rather than taste: a directory heading is `padding 6 + twisty 9 + gap 6`
 * before its icon and `+ icon 15 + gap 6` before its label, so its label sits at 42px — and a
 * nested file at `padding-left: 21` with no twisty put *its* label at 21 + 15 + 6 = 42px too.
 * The two lined up perfectly, which is what "the files are on the same line" describes.
 *
 * So the indent is applied to a **twisty slot every row has**, file rows included, the same
 * arrangement `FileTree` uses. That is what makes a file's icon land under its directory's icon
 * instead of under its directory's text.
 */
export const INDENT = 19

/** Shared, so a default argument does not mint a `Set` on every call. */
const EMPTY: ReadonlySet<string> = new Set()

/**
 * The flat reading: every file, whole path, no headings.
 *
 * Kept as its own function rather than a branch inside [`groupedRows`] so that the flat case
 * cannot acquire a bug from the tree case's arithmetic. It is also the reading a *rename* wants,
 * which is the argument for offering the choice at all: `old → new` across two directories is
 * unreadable when the pane has filed the row under one of them.
 */
export function flatRows(files: readonly ChangedFile[]): FileRow[] {
  return files.map((file) => ({
    kind: 'file' as const,
    id: file.path,
    label: file.oldPath === null ? file.path : `${file.oldPath} → ${file.path}`,
    depth: 0,
    file,
  }))
}

/**
 * The grouped reading: a heading per directory, files under it by basename.
 *
 * **Order is the input's order, not alphabetical.** The backend returns a commit's files in the
 * order the diff produced them, and re-sorting here would mean the grouped and flat readings
 * disagree about which file is first — two views of one commit that cannot be compared by eye.
 * Directories therefore appear in the order their first file does.
 *
 * A file at the repository root gets no heading rather than a heading called `/` or `(root)`.
 * There is no directory to name, and inventing one puts a row on screen that matches nothing the
 * user could search for.
 *
 * A **renamed** file is filed under its *new* directory and keeps the arrow in its label, with
 * the old path shown whole when the two directories differ. Filing it under the old one would
 * hide it from the place the reader is looking for it, and dropping the arrow would silently
 * turn a rename into an ordinary edit.
 */
export function groupedRows(
  files: readonly ChangedFile[],
  /**
   * Directories whose files are hidden.
   *
   * A set of directory paths and not a per-row flag, because the row list is rebuilt from
   * scratch on every render and on every commit: state that lived on a row would be lost the
   * moment the list was recomputed, which is every keystroke in the filter box.
   *
   * Unknown entries are ignored rather than being an error. The set outlives the commit it was
   * built against — a user who folded `crates/cide-git/src` almost certainly wants it folded in
   * the next commit too — so most of it names directories the current commit does not touch.
   */
  collapsed: ReadonlySet<string> = EMPTY,
): FileRow[] {
  const order: string[] = []
  const byDir = new Map<string, ChangedFile[]>()
  for (const file of files) {
    const dir = dirOf(file.path)
    const bucket = byDir.get(dir)
    if (bucket === undefined) {
      order.push(dir)
      byDir.set(dir, [file])
    } else {
      bucket.push(file)
    }
  }

  const rows: FileRow[] = []
  for (const dir of order) {
    const bucket = byDir.get(dir) ?? []
    const depth = dir === '' ? 0 : 1
    const shut = dir !== '' && collapsed.has(dir)
    if (dir !== '') {
      rows.push({
        kind: 'dir',
        id: `dir:${dir}`,
        label: dir,
        depth: 0,
        dir,
        collapsed: shut,
        count: bucket.length,
      })
    }
    // A file at the repository root has no heading, so there is nothing to fold it into — it
    // stays visible however many directories are shut. Folding it under an invented root would
    // hide a file behind a row that does not name it.
    if (shut) continue
    for (const file of bucket) {
      rows.push({
        kind: 'file',
        id: file.path,
        label: labelInDir(file, dir),
        depth,
        file,
      })
    }
  }
  return rows
}

/**
 * What a file is called under its directory heading.
 *
 * The basename, unless it is a rename — then the arrow has to survive, because a rename drawn as
 * a plain basename is indistinguishable from an edit. When the old path is in the *same*
 * directory only the two names are shown, since the heading already says where they are; when it
 * came from elsewhere the old path is shown whole, because `login.rs → login.rs` under one
 * heading would be a rename that appears to change nothing.
 */
function labelInDir(file: ChangedFile, dir: string): string {
  const base = baseOf(file.path)
  if (file.oldPath === null) return base
  return dirOf(file.oldPath) === dir ? `${baseOf(file.oldPath)} → ${base}` : `${file.oldPath} → ${base}`
}

/**
 * The rows for a mode. One entry point, so a caller cannot draw a third arrangement.
 *
 * `collapsed` is ignored in the flat reading and that is not an oversight: there are no headings
 * there, so there is nothing a folded directory could hide behind. Honouring it would make files
 * vanish from a list that offers no way to bring them back.
 */
export function fileRows(
  files: readonly ChangedFile[],
  asTree: boolean,
  collapsed: ReadonlySet<string> = EMPTY,
): FileRow[] {
  return asTree ? groupedRows(files, collapsed) : flatRows(files)
}

/** A directory folded or unfolded, as a new set — the shape a `useState` setter wants. */
export function toggleCollapsed(
  collapsed: ReadonlySet<string>,
  dir: string,
): ReadonlySet<string> {
  const next = new Set(collapsed)
  if (!next.delete(dir)) next.add(dir)
  return next
}
