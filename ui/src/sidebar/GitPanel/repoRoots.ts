/**
 * Where each repository lives on disk, so a repo-relative path can be recognised in an
 * absolute one.
 *
 * # The mismatch this bridges
 *
 * Two vocabularies meet in the diff pane and neither can be changed. `DiffOrigin::Git` spells
 * its path **repo-relative and slash-separated**, because that is how git spells a path and
 * how `git_diff_file` wants it. `cide://session-tool` reports the paths a Claude tool touched
 * exactly as the CLI wrote them — **absolute**, from `tool_input.file_path`. Comparing the two
 * needs the work-tree root that sits between them, and neither event carries it.
 *
 * # Why a registry rather than a fetch
 *
 * The obvious alternative — have each diff tab call `git_status` once to find its root — lost
 * because `git_status` walks the whole work tree to build the commit panel's changes tree, and
 * a restored layout with six diff tabs would run six of those walks at startup to learn six
 * strings. Every `ChangesTree` this window sees already carries `RepoInfo.root` for every repo
 * in the project, and this window sees plenty of them: the panel's own `git_status`, the
 * `cide://git-status` broadcast after any mutation in any window, and every mutation's own
 * reply. So the answer is *noted in passing* and costs no round trip at all.
 *
 * The price is that the map can be empty — a window restored with the sidebar on Files and no
 * git mutation yet has never seen a tree. That case is not a wrong answer, only a coarser one:
 * see {@link touchesFile}, which falls back to matching the path's tail.
 *
 * Module-level and per-window, for the same reason `partialStore` is: the diff pane lives in a
 * workspace tab and the panel that learns the roots lives in the sidebar, and their nearest
 * common React ancestor is the shell. A root is a fact about the filesystem, so a second
 * window learning it independently costs nothing and cannot disagree.
 */
import type { ChangesTree, RepoId } from '@/ipc/generated'

const roots = new Map<RepoId, string>()

/**
 * Record the roots a changes tree names. Cheap enough to call on every tree that arrives.
 *
 * `ChangesTree.repos` is flat — submodules arrive as their own `RepoChanges` rather than
 * nested inside their parent's, which is what lets a monorepo user see what is actually
 * dirty — so there is nothing to recurse into here. The nesting the panel draws is
 * `normalizeStatus`'s doing and happens later.
 */
export function noteRepoRoots(tree: ChangesTree): void {
  for (const repo of tree.repos) roots.set(repo.repo.id, repo.repo.root)
}

/** The absolute work tree of a repo, or `null` if no tree naming it has arrived yet. */
export function repoRoot(repo: RepoId): string | null {
  return roots.get(repo) ?? null
}

/**
 * Whether a list of absolute paths contains this one repo-relative file.
 *
 * Pure, and separate from the map above so that `check-diff-selection.mjs` can exercise both
 * branches without a window.
 *
 * With a `root` this is an equality, which is the whole point: a tool call in another project
 * names paths under another root, so its `src/main.rs` is not this tab's `src/main.rs` and the
 * tab does not refetch. Two projects that genuinely share a work tree compare equal — and
 * should, because the file really did change under this tab.
 *
 * Without a root it falls back to the path's tail. That is deliberately the *loose* direction:
 * the failure it admits is a wasted `git_diff_file` when some other repo happens to hold a
 * file at the same relative path, and the failure it refuses to admit is a diff tab that
 * quietly stops following its own file. A stricter fallback — refuse everything until a root
 * is known — would do exactly that for as long as the sidebar stays on Files.
 *
 * Trailing separators are trimmed off the root because `RepoInfo.root` is canonicalised and a
 * filesystem root (`/`) would otherwise join to `//src/main.rs`.
 */
export function touchesFile(
  touched: readonly string[],
  root: string | null,
  path: string,
): boolean {
  if (path === '') return false
  if (root !== null) {
    const absolute = `${root.replace(/\/+$/, '')}/${path}`
    return touched.includes(absolute)
  }
  const tail = `/${path}`
  return touched.some((p) => p === path || p.endsWith(tail))
}
