/**
 * Which project root a tree row belongs to, and what its path looks like relative to it.
 *
 * A module of its own — pure and **import-free**, like `treeStatus.ts` and
 * `clickSemantics.ts` beside it — because the first version of this rule lived inline in
 * `FileTree`'s context menu and was wrong in a way nothing could see. It read `TreeRow.root`,
 * which is an *index into `Project::roots`*, as though it were the root's path. `path === "0"`
 * is never true, so *Rename…* and *Move to Trash* stayed enabled on a project root for Rust's
 * `check_not_root` to refuse; `path.startsWith("0/")` is never true either, so *Copy Relative
 * Path* copied the absolute path — byte for byte what *Copy Path* one line above it copies.
 * Both type-check. Neither is visible in a screenshot. `check-tree-status.mjs` pins them here.
 *
 * The roots come from the workspace, where they are paths. Nothing in this module knows that;
 * it takes them as strings.
 */

/**
 * The root that holds `path`, or `null` when none does.
 *
 * **Longest match wins.** A project may legitimately be opened over both `~/work/cide` and
 * `~/work/cide/ui`, and under a first-match rule the same file would be named `ui/src/App.tsx`
 * from one row and `src/App.tsx` from another depending on the order the roots happen to be
 * in — which is how people stop trusting *Copy Relative Path* altogether.
 *
 * **Segment-aware.** `~/work/cide-old/x` is not inside `~/work/cide`; a bare `startsWith`
 * says it is, and would then slice the path at the wrong offset and produce `ld/x`.
 *
 * The root itself matches, so [`relativeTo`] can tell "this row *is* a root" from "this row is
 * under one" — they take different menu items.
 */
export function rootOf(path: string, roots: readonly string[]): string | null {
  let best: string | null = null
  for (const root of roots) {
    if (root === '') continue
    if (path !== root && !path.startsWith(`${root}/`)) continue
    if (best === null || root.length > best.length) best = root
  }
  return best
}

/**
 * What *Copy Relative Path* copies.
 *
 * The **absolute** path when no root claims the row, and when the row is a root itself. That
 * is deliberate rather than a fallback to `''`: a root's path relative to itself is the empty
 * string, and an empty clipboard is indistinguishable from a menu item that did nothing.
 */
export function relativeTo(path: string, roots: readonly string[]): string {
  const root = rootOf(path, roots)
  return root === null || root === path ? path : path.slice(root.length + 1)
}

/** Whether this row is one of the project's roots — the rows `fs_rename`/`fs_delete` refuse. */
export function isRootPath(path: string, roots: readonly string[]): boolean {
  return rootOf(path, roots) === path
}

/**
 * Why the disk-changing verbs are refused for this set of rows, or `null`.
 *
 * *Cut*, *Rename…*, *Move to Trash*, *New File in…* and *Paste* all end in a Rust handler that
 * calls `cide_fs::ops::check_within`, and since M13 the file tree draws rows that check cannot
 * pass: the *External Libraries* group's header (a `cide://group/…` sentinel, not absolute) and
 * every dependency source under it (absolute, and outside every root). Both would have been
 * enabled menu items that error — the dead control this panel has already shipped twice — so the
 * refusal is said here, before the click, in the words the menu greys the row with.
 *
 * The two clauses are the same two `check_within` applies, in the same order and for the same
 * reasons, so the menu and the handler cannot drift into two different rules. `writable` is
 * therefore the list Rust checks against — `ProjectFs::writable_paths`, delivered by
 * `fs_writable_roots` — and **not** the project's roots: since M13 the *Scratches* drawer is a
 * directory outside every root that cide may still rename and delete inside, and deriving that
 * here from a second copy of the rule is exactly how the two would come apart. The first is
 * tested by *shape* — "is it absolute" — rather than against a list of `cide://` prefixes, so a
 * synthetic scheme a later group invents is covered the day it exists.
 *
 * One sentence for the whole set, not one per row: the verbs act on the selection as a unit, and
 * a menu that greyed *Move 3 Items to Trash* would have to say which of the three is the problem.
 * Naming the class is the useful half.
 */
export function mutationRefusal(
  paths: readonly string[],
  writable: readonly string[],
): string | null {
  if (paths.length === 0) return null
  if (paths.some((path) => !path.startsWith('/'))) {
    return 'This row is a heading, not a file'
  }
  if (paths.some((path) => rootOf(path, writable) === null)) {
    return 'This file is outside the project, so cide will not move, rename or delete it'
  }
  return null
}

/**
 * Why *New File…*, *New Folder…* and *Paste* are refused **inside** this row's directory, or
 * `null`. Strictly narrower than [`mutationRefusal`], which every caller checks first.
 *
 * One population reaches it: the *Scratches* drawer. Its files are writable — that is the whole
 * point of a scratch, and `mutationRefusal` passes them — but the directory holding them is not
 * a folder anybody organises. It is `$XDG_STATE_HOME/cide/scratches/<blake3 of a path>`, so a
 * `New File in…` item there would name the destination `4f2a9c7b…` and drop an ordinary file
 * into a drawer whose contents are supposed to be scratches. Two facts follow from that and
 * both are the reason this exists rather than the menu simply being widened: the label would be
 * a hash, and the file would be listed as a scratch without being one.
 *
 * `roots`, not the writable set, and that is the whole test: a directory is one the user fills
 * exactly when some project root contains it.
 */
export function creationRefusal(path: string, roots: readonly string[]): string | null {
  if (rootOf(path, roots) !== null) return null
  return 'cide keeps scratch files here — use “New scratch file…” to add one'
}
