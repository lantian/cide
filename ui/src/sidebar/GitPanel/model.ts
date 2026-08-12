/**
 * The git panel's pure core: wire data in, flat rows and tri-state out.
 *
 * Everything here is a plain function over plain data — no React, no IPC, no DOM. That is
 * what lets `ui/scripts/check-git-tree.mjs` run it under node and pin the behaviour that is
 * easy to get subtly wrong: the elided repo level, indentation depth, and a check state
 * that has to distinguish "none of these files" from "some of the hunks of one of them".
 *
 * The tree is flattened to a row list rather than rendered recursively because the
 * checkbox semantics are *not* local: ticking `Changes` has to reach every file below it,
 * including files inside a nested submodule, and a group's own state is a fold over the
 * same set. A flat list with a precomputed descendant-file set makes both O(1) at the
 * point of use and keeps the React component free of tree walking.
 *
 * # The shape this reads
 *
 * `normalizeStatus` takes a `cide_ipc::git::ChangesTree` — `repos[].repo.{id,root,name}`,
 * `repos[].branch`, and the four sibling lists `changelists` / `unversioned` / `ignored` /
 * `conflicts` — and folds it into the `StatusView` the rows are built from. It does **not**
 * take the shape the panel used to assume; see `types.ts` for what that cost.
 *
 * # Repo ids, not paths
 *
 * Every row carries a `RepoId`, because that is what `cmd/git.rs::repo_root` resolves through
 * `cide_git::repo::find`. Passing the work-tree path where a `RepoId` is expected is a
 * `NoSuchRepo` on every command, which is indistinguishable from "the panel does nothing".
 */
import type {
  ChangeEntry,
  ChangelistTarget,
  ChangesTree,
  DiffOpenMode,
  FileState,
  GroupKind,
  GroupView,
  RepoId,
  RepoView,
  StatusView,
} from './types'

/** Tri-state. `partial` is the mock's `–` glyph and ARIA's `mixed`. */
export type CheckState = 'checked' | 'partial' | 'unchecked'

/**
 * `dir` is a folder *inside one group of one repository*.
 *
 * A directory row exists so the tree has something to grab that means "everything under here"
 * — the user asked to drag whole directories between changelists, and a flat list of full paths
 * offers no such row. It is scoped to its group, never above it: the same `src/` that has files
 * in two changelists is two rows, one per list. The alternative — one directory row per repo
 * that gathers every changelist's files under that path — would let a drag move files the user
 * cannot see from the row they grabbed, which is the exact surprise `actOn`'s count in the menu
 * label exists to prevent.
 */
export type RowKind = 'repo' | 'group' | 'dir' | 'file'

/** One 23px line in the tree. */
export interface Row {
  /** Stable across refreshes, which is what lets selection and expansion survive one. */
  id: string
  kind: RowKind
  /** Indent multiplier. 0 is flush with the panel's padding. */
  depth: number
  label: string
  /**
   * The repository this row belongs to — the `RepoId` every `git_*` command takes.
   *
   * Not the work-tree path. `repo_root` resolves an id through `cide_git::repo::find`, so a
   * path here is a `NoSuchRepo` on every command the row can reach.
   */
  repo: RepoId
  /** Repo rows only: the absolute work tree, for the row's tooltip. */
  root?: string
  /**
   * Group rows only: the `GroupView.id` — `cl:fixes`, `unversioned`, `ignored`, `conflicts`.
   *
   * Carried on the row rather than parsed back out of `Row.id`, because the row id is built
   * with a NUL separator that exists precisely so nothing has to be parsed. The context menu
   * needs the raw changelist id to send to `git_changelist_*`, and `changelistIdOf` is the one
   * place that strips the `cl:` prefix.
   */
  group?: string
  /**
   * Which of the four lists this row is in — set on group rows **and on file rows**.
   *
   * The file rows need it because filing an untracked or ignored path into a changelist is a
   * no-op the user cannot see: `cide_git::status` builds its `live` set from paths that are
   * neither `Untracked` nor `Ignored`, so `reconcile` drops the assignment on the very next
   * status walk. The menu therefore offers *Move to Changelist…* only where it does something.
   */
  groupKind?: GroupKind
  /** File rows only. */
  entry?: ChangeEntry
  /**
   * Directory rows only: the repo-relative directory, e.g. `crates/cide-git/src`.
   *
   * The whole path rather than the last segment, because a compacted row (`ui/src/sidebar` on
   * one line) is not reconstructible from `label` plus depth, and the drag ghost names what is
   * being carried.
   */
  path?: string
  /**
   * File rows only: the changelist this file sits in, as *Rust* filed it
   * (`ChangeEntry::changelist`). Commit needs it — committing "the selection" without naming
   * the changelist it came from is how the *other* changelist gets disturbed.
   */
  changelist?: string
  /** Group and repo rows — what the mock right-aligns after the name. */
  count?: number
  /**
   * Group rows only: the active changelist, which is where new changes land and what a
   * commit defaults to. Drawn in `--text` where the other groups are `--dim`.
   */
  active?: boolean
  /** Group and repo rows can collapse; file rows cannot. */
  expandable: boolean
  /**
   * Ids of every *file* row at or below this row.
   *
   * For a file row this is `[id]`. Precomputed during the flatten so toggling a group is a
   * set union rather than another walk, and so a group's tri-state is a fold over an array
   * that is already to hand.
   */
  files: string[]
}

/**
 * Row ids are built from repo ids and paths, so the separator must be a character neither
 * contains. NUL is the one byte a POSIX path is guaranteed not to hold, which makes
 * `${repo}\0${path}` collision-free without escaping. It never reaches the DOM as an
 * attribute — rows are keyed by it in React and looked up in Sets, nothing more.
 */
const SEP = '\u0000'

/** The group id the ignored list always has, so a row id can be recognised without a lookup. */
export const IGNORED_GROUP = 'ignored'

/**
 * Which command a gesture that opens should route to.
 *
 * `gitTreeClick` returns `open` for two different reasons — a double-click, or a single click
 * while a diff is already up — and routing both to `tab_open_diff` is what produced thirty
 * tabs for one walk down a changelist. This is the second half of that decision, and it lives
 * here rather than inline in `ChangesTree` for one reason: `ChangesTree` is a React component
 * with a DOM, so nothing in this repo can execute it, and the mapping was the last unexecuted
 * link between the click rule and the Rust that fixes the bug. `check-git-tree.mjs` imports
 * this under node and runs it.
 *
 * Not folded into `clickSemantics.ts`: that module is shared with the file tree and the search
 * results, and neither of them has a diff to retarget. Not a fourth field on `RowAction` for
 * the same reason.
 */
export function diffOpenMode(gesture: 'single' | 'double'): DiffOpenMode {
  return gesture === 'single' ? 'retarget' : 'open'
}

export function repoRowId(repo: RepoId): string {
  return `${repo}${SEP}repo`
}

export function groupRowId(repo: RepoId, group: string): string {
  return `${repo}${SEP}g${SEP}${group}`
}

/** The id of a file row, and the key under which it is selected. */
export function fileRowId(repo: RepoId, path: string): string {
  return `${repo}${SEP}f${SEP}${path}`
}

/**
 * The id of a directory row.
 *
 * The group is part of it because the row is: `src/` under `Changes` and `src/` under `fixes`
 * are two different rows holding two different sets of files, and one id for both would make
 * expanding one expand the other and — worse — make a drag from one carry the other's files.
 */
export function dirRowId(repo: RepoId, group: string, path: string): string {
  return `${repo}${SEP}d${SEP}${group}${SEP}${path}`
}

/**
 * Is this expanded row id the ignored group of some repository?
 *
 * Drives `includeIgnored` on the next `git_status`. Walking every repo's ignored files is the
 * expensive half of a status on a checkout with a big `target/`, so it is asked for only once
 * the user has actually opened the group — which is a question about *row ids*, since that is
 * all the expansion set holds.
 */
export function isIgnoredGroupRow(id: string): boolean {
  return id.endsWith(`${SEP}g${SEP}${IGNORED_GROUP}`)
}

/**
 * Flatten one status view into rows, honouring collapsed groups.
 *
 * The repo level is elided when there is exactly one repository (§5.3): with a single root and
 * no submodules the panel must look *exactly* like the mock, which has no repo header. A
 * second root — or a submodule, which is also a repository — brings the header back, which is
 * the intended behaviour rather than a layout that is always one level deeper than the design.
 */
export function buildRows(view: StatusView, expanded: ReadonlySet<string>): Row[] {
  const rows: Row[] = []
  const elide = countRepos(view.repos) === 1
  for (const repo of view.repos) walkRepo(rows, repo, elide ? -1 : 0, expanded)
  return rows
}

function countRepos(repos: readonly RepoView[]): number {
  return repos.reduce((n, r) => n + 1 + countRepos(r.children), 0)
}

/**
 * Emit one repository's rows.
 *
 * `depth` is where the repo row itself goes; `-1` means the repo level is elided and its
 * groups start at 0. A repo with nothing under it emits nothing at all — a clean submodule is
 * a row that says nothing and pushes the changes further down the panel.
 */
function walkRepo(
  rows: Row[],
  repo: RepoView,
  depth: number,
  expanded: ReadonlySet<string>,
): void {
  if (countRepoFiles(repo) === 0) return
  const inner = depth + 1
  if (depth >= 0) {
    const id = repoRowId(repo.id)
    rows.push({
      id,
      kind: 'repo',
      depth,
      label: repo.name,
      repo: repo.id,
      root: repo.root,
      expandable: true,
      // Read out of the payload, never back off the rows just emitted. A collapsed group
      // emits no file rows at all, so a rows-derived set would leave an expanded repo
      // whose changelists are shut owning nothing: an unchecked-looking box over ticked
      // files, and a click on it that silently does nothing.
      files: repoFileIds(repo),
      count: countRepoFiles(repo),
    })
    if (!expanded.has(id)) return
  }

  for (const group of repo.groups) {
    const id = groupRowId(repo.id, group.id)
    rows.push({
      id,
      kind: 'group',
      depth: inner,
      label: group.name,
      repo: repo.id,
      group: group.id,
      groupKind: group.kind,
      expandable: true,
      count: group.entries.length,
      ...(group.active ? { active: true } : {}),
      files: group.entries.map((e) => fileRowId(repo.id, e.path)),
    })
    if (!expanded.has(id)) continue
    walkDir(rows, repo.id, group, dirTree(group.entries), inner + 1, expanded)
  }

  // Submodules after the parent's own groups: they are separate repositories with their own
  // index, and burying the parent's changes under them would bury the common case.
  for (const child of repo.children) walkRepo(rows, child, inner, expanded)
}

/**
 * One group's files, arranged by directory.
 *
 * Built per group and never cached: it is a fold over an array the caller already holds, and a
 * cache keyed on a `readonly ChangeEntry[]` would have to be invalidated by identity — which
 * this panel breaks on every refresh anyway, several times a second while an agent edits.
 */
interface DirNode {
  /** Repo-relative directory, e.g. `crates/cide-git/src`. `''` for the group's own root. */
  path: string
  /** What the row shows: the last segment, or several joined when the chain was compacted. */
  label: string
  dirs: DirNode[]
  files: ChangeEntry[]
}

function dirTree(entries: readonly ChangeEntry[]): DirNode {
  const root: DirNode = { path: '', label: '', dirs: [], files: [] }
  for (const entry of entries) {
    const parts = entry.path.split('/')
    // The basename never becomes a directory, so a path with no slash lands straight in the
    // root and the group looks exactly as it did before directories existed.
    parts.pop()
    let node = root
    let prefix = ''
    for (const part of parts) {
      // A leading or doubled slash would otherwise mint a directory called `''`, which draws
      // as a nameless row you can expand. Skipped rather than rejected: the file itself is
      // real, and hiding it would understate what a commit is about to include.
      if (part === '') continue
      prefix = prefix === '' ? part : `${prefix}/${part}`
      const found = node.dirs.find((d) => d.path === prefix)
      if (found === undefined) {
        const made: DirNode = { path: prefix, label: part, dirs: [], files: [] }
        node.dirs.push(made)
        node = made
      } else {
        node = found
      }
    }
    node.files.push(entry)
  }
  compact(root)
  return root
}

/**
 * `crates/cide-git/src` is one row, not three.
 *
 * A chain of directories with one child and no files of its own carries no information per
 * level — three rows and three twisties to reach one file, in a 420px panel where indentation
 * is 14px a level. IDEA compacts the same way. The compacted row keeps the *deepest* path, so
 * its id and the set of files under it are unchanged by the collapsing.
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

/** Every file id at or below a node, in row order. */
function dirFileIds(repo: RepoId, node: DirNode): string[] {
  return [
    ...node.dirs.flatMap((d) => dirFileIds(repo, d)),
    ...node.files.map((e) => fileRowId(repo, e.path)),
  ]
}

/**
 * Emit one directory level: its subdirectories, then its own files.
 *
 * Directories first at every level, which is what every file manager and both other trees in
 * this app do — a folder buried between two files is a folder nobody finds.
 */
function walkDir(
  rows: Row[],
  repo: RepoId,
  group: GroupView,
  node: DirNode,
  depth: number,
  expanded: ReadonlySet<string>,
): void {
  for (const dir of node.dirs) {
    const id = dirRowId(repo, group.id, dir.path)
    rows.push({
      id,
      kind: 'dir',
      depth,
      label: dir.label,
      repo,
      group: group.id,
      groupKind: group.kind,
      path: dir.path,
      expandable: true,
      // Read off the node, not off the rows below: a collapsed directory emits no file rows
      // at all, and a rows-derived set would leave its checkbox unticked over ticked files and
      // its drag carrying nothing. Same rule as the repo row's.
      files: dirFileIds(repo, dir),
    })
    if (!expanded.has(id)) continue
    walkDir(rows, repo, group, dir, depth + 1, expanded)
  }
  for (const entry of node.files) {
    rows.push({
      id: fileRowId(repo, entry.path),
      kind: 'file',
      depth,
      // The basename alone: the directory is the row above, and repeating it on every leaf is
      // what the flat tree did. `FileLabel` no longer draws a dimmed parent for the same reason.
      label: splitPath(entry.path).name,
      repo,
      groupKind: group.kind,
      entry,
      changelist: entry.changelist,
      expandable: false,
      files: [fileRowId(repo, entry.path)],
    })
  }
}

/** Every directory row id in one group, whatever is collapsed. */
function dirRowIdsOf(repo: RepoId, group: GroupView): string[] {
  const out: string[] = []
  const walk = (node: DirNode) => {
    for (const dir of node.dirs) {
      out.push(dirRowId(repo, group.id, dir.path))
      walk(dir)
    }
  }
  walk(dirTree(group.entries))
  return out
}

function repoFileIds(repo: RepoView): string[] {
  const own = repo.groups.flatMap((g) => g.entries.map((e) => fileRowId(repo.id, e.path)))
  return [...own, ...repo.children.flatMap(repoFileIds)]
}

/** Files at or below a repository, its submodules included. The number the mock right-aligns. */
export function countRepoFiles(repo: RepoView): number {
  const own = repo.groups.reduce((n, g) => n + g.entries.length, 0)
  return repo.children.reduce((n, c) => n + countRepoFiles(c), own)
}

/**
 * The tri-state of one row.
 *
 * A file is `partial` when it is selected but only some of it is staged, and that partiality
 * has to climb: a group whose only selected file is half-staged is not a group that is fully
 * checked, and drawing it as `✓` would tell the user the commit contains more than it does.
 */
export function checkState(
  row: Row,
  selected: ReadonlySet<string>,
  partial: (fileId: string) => boolean,
): CheckState {
  if (row.files.length === 0) return 'unchecked'
  let checked = 0
  let mixed = false
  for (const id of row.files) {
    if (selected.has(id)) {
      checked++
      if (partial(id)) mixed = true
    }
  }
  if (mixed) return 'partial'
  if (checked === 0) return 'unchecked'
  return checked === row.files.length ? 'checked' : 'partial'
}

/**
 * Tick or untick a row, with its whole subtree.
 *
 * Untick when anything below is currently on — matching IDEA and every file manager: from
 * `partial`, one click clears rather than completes. The alternative (partial → checked)
 * makes it impossible to clear a large group in one action, which is the common case when
 * a changelist was ticked by mistake.
 */
export function toggleRow(row: Row, selected: ReadonlySet<string>): Set<string> {
  const next = new Set(selected)
  const anyOn = row.files.some((id) => next.has(id))
  for (const id of row.files) {
    if (anyOn) next.delete(id)
    else next.add(id)
  }
  return next
}

/**
 * Drop selections whose file no longer exists, keep the ones that do.
 *
 * Called on every refresh. Without it, reverting a file and re-creating it later would
 * silently arrive pre-ticked, and a selection set that only ever grows would eventually
 * name paths from a repo that has been closed.
 */
export function pruneSelection(
  live: readonly string[],
  selected: ReadonlySet<string>,
): Set<string> {
  const alive = new Set(live)
  return new Set([...selected].filter((id) => alive.has(id)))
}

/** One file, with everything a command needs to act on it. */
export interface FileEntry {
  id: string
  repo: RepoId
  /** The changelist Rust filed this path under. */
  changelist: string
  /** Which of the four lists it arrived in — a conflict must not be committed. */
  kind: GroupKind
  entry: ChangeEntry
}

/**
 * Every file in the tree, regardless of what is collapsed.
 *
 * This, and not the row list, is what commit and the footer count read. Rows only exist
 * for expanded groups, so counting them would drop the ticks inside a collapsed changelist
 * — a commit that silently omits files because a group happened to be shut is the worst
 * bug this panel could have.
 */
export function flatFiles(view: StatusView): FileEntry[] {
  const out: FileEntry[] = []
  const walk = (repo: RepoView) => {
    for (const group of repo.groups) {
      for (const entry of group.entries) {
        out.push({
          id: fileRowId(repo.id, entry.path),
          repo: repo.id,
          changelist: entry.changelist,
          kind: group.kind,
          entry,
        })
      }
    }
    for (const child of repo.children) walk(child)
  }
  for (const repo of view.repos) walk(repo)
  return out
}

/** Every file id in the tree, in row order — what "select all" and a first load use. */
export function allFiles(view: StatusView): string[] {
  return flatFiles(view).map((e) => e.id)
}

/**
 * Is only part of this file staged?
 *
 * `staged` says the index differs from HEAD; a worktree side that is not `unmodified` says the
 * working tree differs from the index. Both at once is precisely "some of this file is in the
 * commit and some is not", which is the leaf-level `–` the tri-state exists for. Derived here
 * rather than sent as a flag because it is a fact about the pair `ChangeEntry` already carries,
 * and a second field saying the same thing is a second field that can disagree.
 */
export function isPartiallyStaged(entry: ChangeEntry): boolean {
  return entry.staged && entry.worktree !== 'unmodified'
}

/** The ids whose file is only partly staged. The input to `checkState`'s `partial`. */
export function partialFiles(view: StatusView): Set<string> {
  return new Set(flatFiles(view).flatMap((e) => (isPartiallyStaged(e.entry) ? [e.id] : [])))
}

/** Every collapsible row id: repo rows, groups, and every directory inside them. "Expand all". */
export function allGroups(view: StatusView): Set<string> {
  const out = new Set<string>()
  const walk = (repo: RepoView) => {
    out.add(repoRowId(repo.id))
    for (const group of repo.groups) {
      out.add(groupRowId(repo.id, group.id))
      for (const id of dirRowIdsOf(repo.id, group)) out.add(id)
    }
    for (const child of repo.children) walk(child)
  }
  for (const repo of view.repos) walk(repo)
  return out
}

/**
 * Does this row id belong to that repository?
 *
 * The separator matters: without it a repo id that is a prefix of another's would put one
 * repo's rows inside the other, which in a multi-repo workspace means the guard bar's
 * "reload" clearing the wrong repo's ticks.
 */
export function inRepo(id: string, repo: RepoId): boolean {
  return id.startsWith(`${repo}${SEP}`)
}

/**
 * What is ticked when the panel first paints.
 *
 * The active changelist and nothing else — changelists are the truth here (ADR 0004), and
 * the acceptance test is "commit one changelist, the other is untouched". Ticking every
 * changelist by default would make that a thing the user has to undo before every commit,
 * which is the point at which people stop reading the tree.
 *
 * Unversioned, ignored and conflicted files are never in it: `git commit` should not sweep in
 * `target/`, and a conflicted path cannot be committed at all. With no changelist marked
 * active — which a repository in staging-area mode reports — every changelist is ticked,
 * because the alternative is a panel that opens with nothing to commit and no clue why.
 */
export function defaultSelection(view: StatusView): Set<string> {
  const out = new Set<string>()
  const walk = (repo: RepoView) => {
    const hasActive = repo.groups.some((g) => g.kind === 'changelist' && g.active)
    for (const group of repo.groups) {
      if (group.kind !== 'changelist') continue
      if (hasActive && !group.active) continue
      for (const entry of group.entries) out.add(fileRowId(repo.id, entry.path))
    }
    for (const child of repo.children) walk(child)
  }
  for (const repo of view.repos) walk(repo)
  return out
}

/**
 * What a freshly arrived payload adds to the ticks and to the open groups.
 *
 * A refresh must not re-tick what the user unticked or re-open what they shut, so "new" means
 * *absent from the previous payload* — which is why both `seen` sets are parameters rather
 * than something derived from `next`.
 *
 * # Why this is a named function and not two loops inside `adopt`
 *
 * It used to be two loops inside the `setSelected`/`setExpanded` updaters in `useGitPanel`,
 * reading the `seenFiles`/`seenGroups` refs that the very same function overwrote a few lines
 * later. React evaluates a state updater eagerly only while the fiber has no other update
 * pending (`dispatchSetStateInternal`, and `enqueueUpdate` marks the fiber synchronously); the
 * panel always has one, because `refresh` calls `setLoading(false)` immediately before
 * `adopt`. So the updaters ran during the *next render* instead — by which time both refs held
 * this payload, every id tested as already-seen, and nothing was ever ticked or expanded. The
 * panel opened with every group collapsed, an empty selection and a dead Commit button.
 *
 * Computed here, from values read at call time, the answer depends on the data rather than on
 * when React decides to run an updater.
 */
export interface Arrivals {
  /** Ids of files that are new *and* belong in the default ticks. */
  files: string[]
  /** Ids of collapsible rows that are new *and* start open. */
  groups: string[]
}

export function arrivals(
  next: StatusView,
  seenFiles: ReadonlySet<string>,
  seenGroups: ReadonlySet<string>,
): Arrivals {
  const defaults = defaultSelection(next)
  return {
    files: allFiles(next).filter((id) => !seenFiles.has(id) && defaults.has(id)),
    groups: [...defaultExpanded(next)].filter((id) => !seenGroups.has(id)),
  }
}

/**
 * Groups that start open. Ignored files are noise until asked for, exactly as in IDEA.
 *
 * Directories start open too, so the panel opens showing the same files it always did — one
 * level further in, with the shared prefix hoisted onto a row of its own. A directory that
 * started shut would hide changes behind a twisty on first paint, which is the one thing a
 * commit tool window must never do.
 */
export function defaultExpanded(view: StatusView): Set<string> {
  const out = new Set<string>()
  const walk = (repo: RepoView) => {
    out.add(repoRowId(repo.id))
    for (const group of repo.groups) {
      if (group.kind === 'ignored') continue
      out.add(groupRowId(repo.id, group.id))
      for (const id of dirRowIdsOf(repo.id, group)) out.add(id)
    }
    for (const child of repo.children) walk(child)
  }
  for (const repo of view.repos) walk(repo)
  return out
}

/**
 * The one status a row's colour is chosen from.
 *
 * Deliberately lossy, and deliberately *only* used for the label: `ChangeEntry` carries both
 * sides because the checkbox is a function of the pair, and `isPartiallyStaged` is what reads
 * the pair. This answers a different question — "what happened to this file" in one word, for
 * one 9px colour — and the index side wins because a file staged as `added` and then edited is
 * still an addition. A conflict outranks both: it is the one state that blocks a commit.
 */
export function entryStatus(entry: ChangeEntry): FileState {
  if (entry.index === 'conflicted' || entry.worktree === 'conflicted') return 'conflicted'
  return entry.index === 'unmodified' ? entry.worktree : entry.index
}

/**
 * The word the footer counts a status in.
 *
 * Git's own adjectives, so they do not pluralise — `2 modified`, not `2 modifieds`. Only
 * `typeChange` needs spelling out; a footer reading `1 typeChange` is a leaked identifier.
 */
const STATUS_WORD: Record<FileState, string> = {
  unmodified: 'unmodified',
  added: 'added',
  modified: 'modified',
  deleted: 'deleted',
  renamed: 'renamed',
  copied: 'copied',
  typeChange: 'type changed',
  untracked: 'untracked',
  ignored: 'ignored',
  conflicted: 'conflicted',
}

/** The order the footer counts statuses in, most-common first so the common line is short. */
const SUMMARY_ORDER: readonly FileState[] = [
  'modified',
  'added',
  'deleted',
  'renamed',
  'copied',
  'typeChange',
  'conflicted',
  'untracked',
  'ignored',
  'unmodified',
]

/**
 * The footer's `2 modified`.
 *
 * With more than one status present the parts join with a middle dot, which is the separator
 * the status bar already uses.
 */
export function summarize(entries: readonly ChangeEntry[]): string {
  const counts = new Map<FileState, number>()
  for (const e of entries) {
    const s = entryStatus(e)
    counts.set(s, (counts.get(s) ?? 0) + 1)
  }
  const parts = SUMMARY_ORDER.flatMap((s) => {
    const n = counts.get(s)
    return n === undefined || n === 0 ? [] : [`${n} ${STATUS_WORD[s]}`]
  })
  return parts.length === 0 ? 'nothing selected' : parts.join(' · ')
}

/** The `ChangeEntry` behind each ticked id — collapsed groups included. */
export function selectedFiles(
  view: StatusView,
  selected: ReadonlySet<string>,
): ChangeEntry[] {
  return flatFiles(view).flatMap((e) => (selected.has(e.id) ? [e.entry] : []))
}

/** One repo's share of a commit. */
export interface CommitUnit {
  /** The `RepoId` `git_commit` takes. */
  repo: RepoId
  paths: string[]
  /** The changelist to commit, or `null` when the ticks span more than one. */
  changelist: string | null
}

/**
 * Split the ticked files by repo, naming the changelist when they all came from one.
 *
 * A multi-repo commit is separate commits, one per repo — git has no other kind, and a
 * submodule is its own repo, so a tick inside one is its own commit too. `null` for a mixed
 * selection is deliberate: the backend then commits exactly the paths given and leaves every
 * changelist's membership alone, which is the only honest answer to "commit these files, which
 * came from two changelists".
 */
export function commitUnits(view: StatusView, selected: ReadonlySet<string>): CommitUnit[] {
  const byRepo = new Map<RepoId, { paths: string[]; lists: Set<string> }>()
  for (const entry of flatFiles(view)) {
    if (!selected.has(entry.id)) continue
    const unit = byRepo.get(entry.repo) ?? { paths: [], lists: new Set<string>() }
    unit.paths.push(entry.entry.path)
    unit.lists.add(entry.changelist)
    byRepo.set(entry.repo, unit)
  }
  return [...byRepo].map(([repo, u]) => ({
    repo,
    paths: u.paths,
    changelist: u.lists.size === 1 ? ([...u.lists][0] ?? null) : null,
  }))
}

/** Split a repo-relative path into the name the row shows and its dimmed parent. */
export function splitPath(path: string): { name: string; dir: string } {
  const at = path.lastIndexOf('/')
  return at < 0 ? { name: path, dir: '' } : { name: path.slice(at + 1), dir: path.slice(0, at) }
}

/** Every repository in the view, roots before their submodules — what per-repo calls iterate. */
export function allRepos(view: StatusView): RepoView[] {
  const out: RepoView[] = []
  const walk = (repo: RepoView) => {
    out.push(repo)
    for (const child of repo.children) walk(child)
  }
  for (const repo of view.repos) walk(repo)
  return out
}

/** The repository a row belongs to, or `undefined` if the view moved under it. */
export function repoOf(view: StatusView, id: RepoId): RepoView | undefined {
  return allRepos(view).find((r) => r.id === id)
}

// --- changelists ---------------------------------------------------------------------------

/**
 * The prefix `normalizeChangelist` puts on a changelist's group id.
 *
 * It exists so a user-created list called `ignored` cannot collide with the ignored sibling
 * list's row id, and it means every group id that reaches a `git_changelist_*` command has to
 * come back off. That stripping happens in exactly one place — `changelistIdOf` — because the
 * failure mode of getting it wrong is a `NoSuchChangelist` on a menu item that looks fine.
 */
export const CHANGELIST_PREFIX = 'cl:'

/** The changelist id behind a group id, or `null` when the group is not a changelist. */
export function changelistIdOf(group: string | undefined): string | null {
  if (group === undefined || !group.startsWith(CHANGELIST_PREFIX)) return null
  return group.slice(CHANGELIST_PREFIX.length)
}

/** The default list. Rust refuses to delete it (`GitError::DefaultChangelist`). */
export const DEFAULT_CHANGELIST = 'default'

/** Every changelist in one repository, in the order Rust listed them, default first. */
export function changelistsOf(view: StatusView, repo: RepoId): ChangelistTarget[] {
  const found = repoOf(view, repo)
  if (found === undefined) return []
  return found.groups.flatMap((group) => {
    const id = changelistIdOf(group.id)
    return id === null
      ? []
      : [{ id, name: group.name, active: group.active, count: group.entries.length }]
  })
}

/** One group by its `GroupView.id` — what a revert, a shelve or a group menu acts on. */
export function groupOf(view: StatusView, repo: RepoId, group: string): GroupView | undefined {
  return repoOf(view, repo)?.groups.find((g) => g.id === group)
}

/*
 * `actOn` used to live here: "which files a menu gesture on this row acts on". It is now
 * `dragDrop.ts::grab`, because a *drag* from a row has to answer exactly the same question and
 * two functions answering it separately is how the menu and the pointer come to disagree about
 * what the user is pointing at. The rule it carried is unchanged and still pinned by
 * `check-git-tree.mjs`: the clicked row alone, unless that row is ticked, and then the ticks in
 * the same repository and the same list kind.
 */

/**
 * The id of the changelist called `name` in `repo`, from a tree Rust just answered with.
 *
 * `git_changelist_create` returns the new `ChangesTree` rather than the id it minted, and the
 * id is a slug of the name with a collision suffix (`my-list`, `my-list-2`) that the frontend
 * must not try to reproduce — two windows creating lists in the same repo would guess the same
 * one. Looking the name up in the answer is exact, and it is the only step between "create"
 * and "move these files into it" in the chooser's inline-create path.
 */
export function findChangelistId(
  tree: ChangesTree,
  repo: RepoId,
  name: string,
): string | null {
  return changelistsOf(viewOf(tree), repo).find((l) => l.name === name)?.id ?? null
}

// --- normalisation ----------------------------------------------------------------------

const FILE_STATES: readonly string[] = [
  'unmodified',
  'added',
  'modified',
  'deleted',
  'renamed',
  'copied',
  'typeChange',
  'untracked',
  'ignored',
  'conflicted',
]

/**
 * Turn a `git_status` payload into the view the rows are built from.
 *
 * The panel's single trust boundary, and the place the empty-panel bug lived. It validates
 * rather than casts — anything unrecognised is dropped and the worst outcome is a missing row
 * — but it validates against `cide_ipc::git::ChangesTree`, which is what Rust actually sends.
 * The previous version required a top-level `root` that no payload has ever carried, so every
 * repository failed the check and the panel was empty against every real repository.
 *
 * `raw as ChangesTree` is still not the answer: that turns a field rename into a TypeError
 * inside render, which in React 19 unmounts the whole window rather than the panel. The fix
 * for the class of bug is that the shape validated here is now the *generated* one, so a
 * rename fails `pnpm exec tsc` before it can fail at run time.
 */
export function normalizeStatus(raw: unknown): StatusView {
  if (!isRecord(raw) || !Array.isArray(raw['repos'])) return { repos: [] }

  const flat = raw['repos'].flatMap((r) => {
    const repo = normalizeRepo(r)
    return repo ? [repo] : []
  })

  // Re-nest submodules under the repository that contains them. `RepoInfo::parent` is a
  // `RepoId`, so one pass over the flat list is enough. A parent that is not in the payload —
  // a submodule whose root was closed between the walk and here — leaves its child at the top
  // level rather than dropping it: an orphaned row is odd, a vanished change is dangerous.
  const byId = new Map<RepoId, RepoView>(flat.map(({ view }) => [view.id, view]))
  const roots: RepoView[] = []
  for (const { view, parent } of flat) {
    const owner = parent === null ? undefined : byId.get(parent)
    if (owner !== undefined && owner !== view) owner.children.push(view)
    else roots.push(view)
  }
  return { repos: roots }
}

/**
 * One repository, plus the parent id the nesting pass needs.
 *
 * The parent travels beside the view rather than on it because nothing downstream reads it —
 * a `RepoView.parent` field would be a second source of truth for a tree that `children`
 * already describes, and the two could disagree.
 */
function normalizeRepo(raw: unknown): { view: RepoView; parent: RepoId | null } | null {
  if (!isRecord(raw)) return null
  const info = raw['repo']
  if (!isRecord(info)) return null
  const id = str(info['id'])
  const root = str(info['root'])
  // No id, no commands: every `git_*` handler resolves a `RepoId`, so a repo without one is a
  // set of rows whose every button would fail. The root is what the tooltip and the file tree
  // agree on, and its absence means the payload is not a `RepoInfo` at all.
  if (id === undefined || root === undefined) return null

  const changelists = Array.isArray(raw['changelists'])
    ? raw['changelists'].flatMap((c, i) => {
        const group = normalizeChangelist(c, i)
        return group ? [group] : []
      })
    : []

  const groups: GroupView[] = [
    // Conflicts first: they block the commit, and a group that has to be scrolled to is a
    // group that gets committed around.
    sidecarGroup('conflicts', 'Merge Conflicts', 'conflicts', raw['conflicts']),
    ...changelists,
    sidecarGroup('unversioned', 'Unversioned Files', 'unversioned', raw['unversioned']),
    sidecarGroup(IGNORED_GROUP, 'Ignored Files', 'ignored', raw['ignored']),
    /*
     * Empty *sibling* lists are dropped; an empty **changelist** is kept.
     *
     * The three sibling lists are derived from the walk, so an empty one is a group about
     * nothing. A changelist is a thing the user made, and dropping it here deleted the whole
     * create half of this feature: `git_changelist_create` answers with a tree in which the
     * new list is necessarily empty, so the list vanished before it could be drawn, before
     * `changelistsOf` could offer it as a move target, and before `findChangelistId` could
     * read its id back — which made the chooser's inline *Create and move* fail every single
     * time with "was created but the files did not move". It also meant a list could never be
     * moved *back* into once its last file left it.
     *
     * A clean repository still renders nothing: `walkRepo` emits no rows for a repo with no
     * files at all, whatever groups it carries.
     */
  ].flatMap((g) => (g.entries.length > 0 || g.kind === 'changelist' ? [g] : []))

  const view: RepoView = {
    id,
    root,
    name: str(info['name']) ?? root.split('/').filter(Boolean).pop() ?? root,
    branch: normalizeBranch(raw['branch']),
    isSubmodule: bool(info['isSubmodule']) ?? false,
    indexChangedExternally: bool(raw['indexChangedExternally']) ?? false,
    useStagingArea: bool(raw['useStagingArea']) ?? false,
    groups,
    children: [],
  }
  return { view, parent: str(info['parent']) ?? null }
}

/**
 * A changelist becomes a group.
 *
 * The id is prefixed so it cannot collide with the three sibling lists: a user is free to
 * create a changelist called `ignored`, and two groups sharing a row id in one repo is a
 * checkbox that ticks the wrong files.
 */
function normalizeChangelist(raw: unknown, index: number): GroupView | null {
  if (!isRecord(raw)) return null
  const name = str(raw['name'])
  if (name === undefined) return null
  return {
    id: `cl:${str(raw['id']) ?? String(index)}`,
    name,
    kind: 'changelist',
    active: bool(raw['active']) ?? false,
    entries: normalizeEntries(raw['changes']),
  }
}

function sidecarGroup(id: string, name: string, kind: GroupKind, raw: unknown): GroupView {
  return { id, name, kind, active: false, entries: normalizeEntries(raw) }
}

function normalizeEntries(raw: unknown): ChangeEntry[] {
  if (!Array.isArray(raw)) return []
  return raw.flatMap((e) => {
    const entry = normalizeEntry(e)
    return entry ? [entry] : []
  })
}

function normalizeEntry(raw: unknown): ChangeEntry | null {
  if (!isRecord(raw)) return null
  const path = str(raw['path'])
  if (path === undefined) return null
  return {
    path,
    origPath: str(raw['origPath']) ?? null,
    // An unknown state is shown as `modified` rather than dropped: the file is genuinely
    // changed, and hiding a row would understate what a commit is about to include.
    index: state(raw['index']) ?? 'modified',
    worktree: state(raw['worktree']) ?? 'unmodified',
    staged: bool(raw['staged']) ?? false,
    binary: bool(raw['binary']) ?? false,
    submodule: bool(raw['submodule']) ?? false,
    changelist: str(raw['changelist']) ?? '',
  }
}

/**
 * A branch is always present in a real payload, so this only ever fills in for a truncated
 * one — and it fills in with `unborn`, which is the state that disables Amend. Guessing a
 * committable branch for a payload we could not read would enable the one control that
 * rewrites history.
 */
function normalizeBranch(raw: unknown): RepoView['branch'] {
  const r = isRecord(raw) ? raw : {}
  return {
    head: str(r['head']) ?? '',
    detached: bool(r['detached']) ?? false,
    upstream: str(r['upstream']) ?? null,
    ahead: num(r['ahead']) ?? 0,
    behind: num(r['behind']) ?? 0,
    operation: str(r['operation']) ?? null,
    unborn: bool(r['unborn']) ?? true,
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined
}

function bool(v: unknown): boolean | undefined {
  return typeof v === 'boolean' ? v : undefined
}

function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined
}

/**
 * A `FileState`, or `undefined` for anything else.
 *
 * Deliberately *not* accepting snake_case. An enum whose Rust side forgets
 * `rename_all = "camelCase"` leaks `type_change`, and that is a Rust bug with a one-line fix;
 * teaching the frontend to accept both spellings would hide it and leave the two casings alive
 * forever.
 */
function state(v: unknown): FileState | undefined {
  return typeof v === 'string' && FILE_STATES.includes(v) ? (v as FileState) : undefined
}

/**
 * The wire tree, for callers that hold a typed `ChangesTree` rather than an unknown payload.
 *
 * `cide://git-status` arrives already typed by `events.onGitStatus`, and casting it back to
 * `unknown` only to re-validate it is the kind of round trip that looks like distrust of the
 * type system. It is the same function underneath.
 */
export function viewOf(tree: ChangesTree): StatusView {
  return normalizeStatus(tree)
}
