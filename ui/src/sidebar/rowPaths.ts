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
