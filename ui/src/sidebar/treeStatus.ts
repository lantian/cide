/**
 * Resolving one file-tree row's git status from the per-path map.
 *
 * Pure functions over plain data — no React, no IPC, no DOM — so that
 * `ui/scripts/check-tree-status.mjs` can run them under node. What is pinned there is the
 * half of the rule that lives on this side of the wire and is invisible in a screenshot:
 * inheritance.
 *
 * # Why a row is not simply a map lookup
 *
 * `git_tree_status` deliberately does not recurse into untracked or ignored directories. A
 * freshly cloned repository has one untracked `target/` and one untracked `src/newfeature/`;
 * enumerating them would put 100k entries on the wire to say one thing. So libgit2 reports
 * the *directory*, and the 5,000 rows inside it have no entry of their own.
 *
 * That makes the lookup two steps: an exact hit, and failing that the **nearest ancestor**
 * that has one. Which ancestor statuses a row inherits is the whole content of this file:
 *
 *   * `untracked` and `ignored` are *container* statuses. Everything under such a directory
 *     shares its fate — git has never heard of any of it either. `ignored` is the one that
 *     earns its keep now that the tree can be told to *show* ignored files
 *     (`ExplorerSettings.showIgnoredFiles`): `target/` arrives as a single entry and the
 *     200,000 rows inside it take their olive tint from this inheritance and from nothing
 *     else. Enumerating them on the wire is not an option, so this is where that colour
 *     comes from.
 *   * `modified` is a *rollup* mark, put on every ancestor of every change so that a folder
 *     shows as dirty when something under it is (IDEA's behaviour, and requirement 3). It
 *     must never be inherited downwards, or one edited file would paint every one of its
 *     siblings blue.
 *
 * Because the rollup marks every ancestor up to the project root, the nearest ancestor with
 * an entry is always decisive: there is no case where a `modified` mark hides an `untracked`
 * one further up, since nothing inside an unrecursed directory produces marks at all.
 */
import type { TreeStatus, TreeStatusMap } from '../ipc/generated'

/** The map before the first `git_tree_status` lands, and whenever git is unavailable. */
export const NO_STATUS: TreeStatusMap = { statuses: {}, truncated: false }

/**
 * How far up the ancestor chain the resolver will walk.
 *
 * A bound rather than a `while (true)`: the keys are strings that arrived over IPC, and a
 * malformed one must cost a wrong tag on one row rather than a hung render. 64 is well past
 * any real path — the deepest thing in this repository is 6 components.
 */
const MAX_ANCESTORS = 64

/**
 * The statuses a row inherits from a directory above it.
 *
 * A `Set` rather than a comparison chain so that adding a variant to the Rust enum is one
 * edit here and the decision is greppable from both sides.
 */
const INHERITED: ReadonlySet<TreeStatus> = new Set<TreeStatus>(['untracked', 'ignored'])

/**
 * The path separator. Linux-only, like the rest of this app — the watcher is inotify, the
 * shell integration is signal-based and the window layer is GTK — so the alternative is a
 * branch that could never be exercised.
 */
const SEP = '/'

/**
 * One row's status. `clean` for anything the map does not cover, which is most rows.
 *
 * Takes the `statuses` record rather than the whole `TreeStatusMap` so that the caller
 * passes one stable reference and a `truncated` flag flipping does not look like a change of
 * every row.
 */
export function statusAt(
  statuses: TreeStatusMap['statuses'],
  path: string,
): TreeStatus {
  const own = statuses[path]
  if (own !== undefined) return own

  let at = path
  for (let step = 0; step < MAX_ANCESTORS; step++) {
    const cut = at.lastIndexOf(SEP)
    // `<= 0` and not `< 0`: at `/etc` the separator is at index 0, and slicing there would
    // produce the empty string and then loop on it forever.
    if (cut <= 0) return 'clean'
    at = at.slice(0, cut)

    const ancestor = statuses[at]
    if (ancestor === undefined) continue
    // The nearest ancestor with an entry decides, whichever way it decides. Continuing past
    // a `modified` mark would find the rollup on the next folder up and the next, all the way
    // to the root, and answer the same thing for every row in the project.
    return INHERITED.has(ancestor) ? ancestor : 'clean'
  }
  return 'clean'
}

/**
 * The letter this row shows in the 9px tag column. Empty for a row that has none.
 *
 * Two rules that are decisions rather than transcription, stated here where they can be
 * tested under node:
 *
 * **Directories get the colour and no letter.** That is IDEA's treatment — a folder
 * containing changes is tinted, never lettered — and it is what keeps the tag column meaning
 * "this file", rather than filling a deep tree with `M`s that only ever say "something under
 * here". The mock draws letters on file rows only.
 *
 * **`untracked` and `ignored` have no letter either**, and for `ignored` that is now a load-
 * bearing decision rather than a transcription of the mock. The tag column answers one
 * question — *what will git do with this file when you commit* — and M, A and D are its three
 * answers. The answer for an ignored file is "nothing", which is the absence of a letter, not a
 * fourth one; inventing an `I` or a `?` would put a glyph on every row of a shown `target/` and
 * make three meanings compete in a 9px column. So `ignored` is carried entirely by colour —
 * `--ignored`, this palette's muted olive-brown, IDEA's treatment — and by the `aria-label` the
 * tag span keeps whether or not it draws a glyph. The two signals cannot collide, either:
 * `git_tree_status` classifies ignored first, so a path is never both ignored and modified.
 */
export function letterFor(status: TreeStatus, isDir: boolean): string {
  if (isDir) return ''
  return LETTERS[status] ?? ''
}

const LETTERS: Readonly<Partial<Record<TreeStatus, string>>> = {
  modified: 'M',
  added: 'A',
  deleted: 'D',
  // `!`, and it is the one letter here that is not in the mock. (M20) The rule above says the
  // column answers *what will git do with this file when you commit* — and for a conflicted
  // path the answer is "refuse", which is a fourth answer and not the absence of one. It is
  // also the only status whose colour alone would be too quiet: red on a name reads as an
  // error somewhere, `!` reads as an error *here*.
  conflicted: '!',
}
