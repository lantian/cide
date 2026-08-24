/**
 * The details pane's changed-file list, flat or as a real directory tree.
 *
 * Import-free, and compiled standalone by `ui/scripts/check-log.mjs`. Tree building is the kind
 * of code that is obviously right and quietly wrong — the cases that break it are a file at the
 * root, two files whose directories share a prefix but not a segment (`src/app` and `src/apple`),
 * and a rename whose two halves live in different directories. None of those are reachable from a
 * component test.
 *
 * # It is a tree, not a list of directories
 *
 * Until M27 this bucketed files by their whole directory path and drew one row per bucket, which
 * looks like a tree in a commit that touches two directories and stops looking like one the
 * moment it touches four: `ui/src/panes` and `ui/src/sidebar` were two unrelated top-level rows
 * with no shared parent, in a pane whose whole job is *which areas did this touch*. Nesting was
 * not merely absent — it was unrepresentable, because a bucket key has no parent.
 *
 * So this builds the same `DirNode` tree `sidebar/GitPanel/model.ts` builds, walks it the same
 * way, and emits the same three rules:
 *
 * - **Single-child chains are compacted.** `crates/cide-git/src` is one row, not three, because
 *   three rows of nothing but indentation carry no information per level. The compacted row keeps
 *   the *deepest* path, so its id and the set of files under it are unchanged by the collapsing —
 *   which is what lets a fold survive the commit it was made in.
 * - **Directories before files at every level.** What every file manager and both other trees in
 *   this app do; a folder buried between two files is a folder nobody finds. It is the one place
 *   the row order is not the input's, and it is structural rather than a sort.
 * - **Otherwise the input's order**, never alphabetical — see [`groupedRows`].
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
 * The tree reading: a row per directory level, files under their own directory by basename.
 *
 * **Order is the input's order, not alphabetical.** The backend returns a commit's files in the
 * order the diff produced them, and re-sorting here would mean the tree and flat readings
 * disagree about which file is first — two views of one commit that cannot be compared by eye.
 * Directories therefore appear in the order their first file does, at every level. The one
 * departure is that a level draws its directories before its own files: see the header.
 *
 * A file at the repository root gets no heading rather than a heading called `/` or `(root)`.
 * There is no directory to name, and inventing one puts a row on screen that matches nothing the
 * user could search for. It is also why the root is the one node whose *own* row is never drawn.
 *
 * A **renamed** file is filed under its *new* directory and keeps the arrow in its label, with
 * the old path shown whole when the two directories differ. Filing it under the old one would
 * hide it from the place the reader is looking for it, and dropping the arrow would silently
 * turn a rename into an ordinary edit.
 */
export function groupedRows(
  files: readonly ChangedFile[],
  /**
   * Directories whose contents are hidden.
   *
   * A set of directory paths and not a per-row flag, because the row list is rebuilt from
   * scratch on every render and on every commit: state that lived on a row would be lost the
   * moment the list was recomputed, which is every keystroke in the filter box.
   *
   * The path is a compacted node's **deepest** segment — `crates/cide-git/src`, never the
   * `crates` this commit did not draw a row for — because that is what the row itself publishes
   * as its `dir`. Folding one hides everything below it, nested directories included: a fold
   * that left grandchildren on screen would be a disclosure that discloses nothing.
   *
   * Unknown entries are ignored rather than being an error. The set outlives the commit it was
   * built against — a user who folded `crates/cide-git/src` almost certainly wants it folded in
   * the next commit too — so most of it names directories the current commit does not touch.
   */
  collapsed: ReadonlySet<string> = EMPTY,
): FileRow[] {
  const rows: FileRow[] = []
  walk(rows, dirTree(files), 0, collapsed)
  return rows
}

/**
 * One level of the tree: a directory, what it holds, and where it sits.
 *
 * The same shape `sidebar/GitPanel/model.ts` uses, deliberately — the two panes draw the same
 * picture of the same repository and a second arrangement of the same data would be a second
 * place for the compaction rule to drift. It is not *imported* from there because that module
 * reaches the generated DTOs and this one is compiled standalone by `check-log.mjs`.
 */
interface DirNode {
  /** Repo-relative directory, e.g. `crates/cide-git/src`. `''` for the root, which draws no row. */
  path: string
  /** What the row shows: the last segment, or several joined when the chain was compacted. */
  label: string
  dirs: DirNode[]
  files: ChangedFile[]
}

/** Fold the paths into a tree, then compact its single-child chains. */
function dirTree(files: readonly ChangedFile[]): DirNode {
  const root: DirNode = { path: '', label: '', dirs: [], files: [] }
  /*
   * Prefix to node, for the whole build. The obvious `node.dirs.find(d => d.path === prefix)` is
   * a linear scan of the siblings for *every segment of every path*, which is quadratic in the
   * width of a directory — and this runs on every render of a pane whose list can be the four
   * hundred files of a release merge. Insertion order is unchanged: the map only answers "have I
   * made this one already", so directories still appear in the order their first file did.
   */
  const index = new Map<string, DirNode>()
  for (const file of files) {
    const parts = file.path.split('/')
    // The basename never becomes a directory, so a path with no slash lands straight in the root
    // and reads exactly as it did before directories existed.
    parts.pop()
    let node = root
    let prefix = ''
    for (const part of parts) {
      // A leading or doubled slash would otherwise mint a directory called `''`, which draws as
      // a nameless row you can fold. Skipped rather than rejected: the file itself is real, and
      // hiding it would understate what the commit touched.
      if (part === '') continue
      prefix = prefix === '' ? part : `${prefix}/${part}`
      const found = index.get(prefix)
      if (found === undefined) {
        const made: DirNode = { path: prefix, label: part, dirs: [], files: [] }
        node.dirs.push(made)
        index.set(prefix, made)
        node = made
      } else {
        node = found
      }
    }
    node.files.push(file)
  }
  compact(root)
  return root
}

/**
 * `crates/cide-git/src` is one row, not three.
 *
 * A chain of directories with one child and no files of its own carries no information per
 * level — three rows and three twisties to reach one file, at 19px of indent each. IDEA compacts
 * the same way and so does the git panel, whose note this is copied from. The compacted row keeps
 * the **deepest** path, so its id, its fold and the set of files under it are unchanged by the
 * collapsing.
 *
 * This is also why a commit touching only `crates/cide-git/src/` still draws one heading and then
 * its files rather than three headings: the chain above it says nothing this list does not.
 */
function compact(node: DirNode): void {
  node.dirs = node.dirs.map((dir) => {
    let at = dir
    while (at.files.length === 0 && at.dirs.length === 1) {
      const only = at.dirs[0]
      if (only === undefined) break
      at = { path: only.path, label: `${at.label}/${only.label}`, dirs: only.dirs, files: only.files }
    }
    compact(at)
    return at
  })
}

/** How many files sit at or below a node — what a folded heading says it is hiding. */
function countFiles(node: DirNode): number {
  let total = node.files.length
  for (const dir of node.dirs) total += countFiles(dir)
  return total
}

/**
 * Emit one level: its subdirectories (each followed by everything under it), then its own files.
 *
 * `depth` is where *this node's children* are drawn, which is why the root is walked at 0 and its
 * own files land at 0 beside the top-level headings. A folded directory keeps its row and emits
 * nothing below it — dropping the heading too would leave no way to unfold it.
 */
function walk(
  rows: FileRow[],
  node: DirNode,
  depth: number,
  collapsed: ReadonlySet<string>,
): void {
  for (const dir of node.dirs) {
    const shut = collapsed.has(dir.path)
    rows.push({
      kind: 'dir',
      id: `dir:${dir.path}`,
      label: dir.label,
      depth,
      dir: dir.path,
      collapsed: shut,
      // Read off the node and not off the rows below it: a folded directory emits no rows at all,
      // and a rows-derived count would say `0` for the thing it is hiding. Same rule as the git
      // panel's directory row.
      count: countFiles(dir),
    })
    if (shut) continue
    walk(rows, dir, depth + 1, collapsed)
  }
  // A file at the repository root has no heading, so there is nothing to fold it into — it stays
  // visible however many directories are shut. Folding it under an invented root would hide a
  // file behind a row that does not name it.
  for (const file of node.files) {
    rows.push({
      kind: 'file',
      id: file.path,
      label: labelInDir(file, node.path),
      depth,
      file,
    })
  }
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
