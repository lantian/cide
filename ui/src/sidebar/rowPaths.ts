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
 * Where each segment of the status bar's path trail would take you, or `null`. (M16)
 *
 * `crates › cide-core › src › lib.rs › impl Parser › parse` is drawn by `chrome/StatusBar.tsx`
 * as a flat list of crumbs, and the user asked for the ones that name a real place to be
 * clickable. The half that is not obvious is which ones those are, and the answer has to be
 * given **before** the click: a crumb that looks live and then reports *"not in this project's
 * file tree"* is the dead control this project has now shipped nineteen times, and the user
 * ruled that notice out by name. So an inert crumb is drawn inert, and this is the function that
 * decides which.
 *
 * # The rule
 *
 * A segment is revealable exactly when its absolute path is at or under a **reveal root** — a
 * path the file tree can hang a row from. That is [`rootOf`]'s longest-match containment, the
 * same test `cide_fs::groups::Groups::owner_of` applies on the Rust side and says so in as many
 * words. `roots` comes from `fs_reveal_roots`: the project's roots, plus each synthetic group's
 * top-level children. Three populations fall out of it, and all three are what the tree draws:
 *
 * * **A project file.** The trail is drawn relative to the root (`statusReadout::pathTrail`), so
 *   every segment is under it and every one is revealable.
 * * **A library source.** The reveal roots are the *package* directories —
 *   `…/registry/src/index.crates.io-<hash>/serde-1.0.229`, `…/pkg/mod/github.com/foo/bar@v1.2.3`,
 *   `<sysroot>/lib/rustlib/src/rust/library`. So `serde-1.0.229 › src › de › mod.rs` is live and
 *   `home`, `.cargo`, `registry`, `src`, `index.crates.io-…` are not, which is the report this
 *   came from. **This cannot be derived here from a depth rule over the dependency caches** —
 *   the SDK's directory comes out of `rustc --print sysroot` — which is why the list is asked for
 *   rather than computed.
 * * **A file under nothing** — opened through the out-of-project confirmation, or gitignored.
 *   Nothing is revealable, because `fs_reveal` is index-only and would answer `None`.
 *
 * # What this is, precisely: a necessary condition, not a sufficient one
 *
 * Containment is the whole of what a pure function can know. Inside a reveal root, whether a
 * *particular* directory has a row also depends on the project's ignore rules — `target/debug`
 * is under the root and has no row — so a click on a crumb of a gitignored path still reports
 * itself the way ⌃⇧E already does for the file. That residue is uniform rather than misleading:
 * if the file is ignored, so is every ancestor crumb of it, and the trail reads as one live run
 * or none. What this closes is the case that is *structurally* unreachable and was being offered
 * anyway.
 *
 * # Arguments, and why they are these
 *
 * `file` is the buffer's absolute path; `pathCount` is how many leading entries of the trail are
 * path segments. The trail alone is not enough for either job: it may be root-relative (so the
 * crumbs cannot be joined into a path), and its tail is `mod › impl › fn` (so a symbol named
 * `src` would be tested as a directory). `StatusBar` carries both on the readout claim for
 * exactly this call — before it did, the comment in that file said plainly that it "does not know
 * where the path ends and the symbols begin… no crumb is clickable yet".
 *
 * Crumbs at or past `pathCount` are symbols and answer `null` unconditionally. Going somewhere is
 * a thing a *symbol* crumb could plausibly do — IDEA opens File Structure at its siblings — and
 * that is a different gesture with a different target, deliberately not smuggled in here.
 */
export function crumbTargets(
  file: string,
  trailLength: number,
  pathCount: number,
  roots: readonly string[],
): (string | null)[] {
  const out: (string | null)[] = new Array<string | null>(trailLength).fill(null)
  // Where each component of `file` ends, so a crumb's path is a *slice of the original string*.
  // Rejoining split components would have to invent a separator and would lose a leading `/` or
  // a drive letter; slicing cannot, whatever the path looked like going in.
  const ends: number[] = []
  for (let i = 1; i < file.length; i += 1) {
    const prev = file[i - 1]
    if ((file[i] === '/' || file[i] === '\\') && prev !== '/' && prev !== '\\') ends.push(i)
  }
  const last = file[file.length - 1]
  if (file.length > 0 && last !== '/' && last !== '\\') ends.push(file.length)

  // More path crumbs than the file has components means the two ends have come apart — a trail
  // published against one buffer and a `file` from another. Answering "nothing is revealable" is
  // the honest degradation; guessing an alignment would produce a crumb that opens the wrong row.
  if (pathCount > ends.length) return out
  /*
   * How many leading components of `file` the trail does not draw, because it is root-relative.
   *
   * **This is where the symbol exclusion actually lives**, and it is worth saying so rather than
   * letting the loop bound below take the credit: `pathCount` is what anchors crumb 0 to a
   * component of the path, so crumb *i* can only ever address `ends[skipped + i]`, and for
   * `i >= pathCount` that index is past the end. Bound the loop by `trailLength` instead and
   * nothing changes; compute this from `trailLength` — which is what you get if nobody threads
   * the split index through — and the whole trail shifts left, so `parse` resolves to `lib.rs`
   * and reveals it. `check-tree-status.mjs` drives that mutation, not the bound.
   */
  const skipped = ends.length - pathCount
  for (let i = 0; i < Math.min(pathCount, trailLength); i += 1) {
    const at = ends[skipped + i]
    if (at === undefined) continue
    const path = file.slice(0, at)
    out[i] = rootOf(path, roots) === null ? null : path
  }
  return out
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
