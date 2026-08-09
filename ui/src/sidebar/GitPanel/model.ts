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
 * including files inside a nested submodule group, and a group's own state is a fold over
 * the same set. A flat list with a precomputed descendant-file set makes both O(1) at the
 * point of use and keeps the React component free of tree walking.
 */
import type { ChangeFile, ChangeGroup, ChangesTree, FileStatus, RepoChanges } from './types'

/** Tri-state. `partial` is the mock's `–` glyph and ARIA's `mixed`. */
export type CheckState = 'checked' | 'partial' | 'unchecked'

export type RowKind = 'repo' | 'group' | 'file'

/** One 23px line in the tree. */
export interface Row {
  /** Stable across refreshes, which is what lets selection and expansion survive one. */
  id: string
  kind: RowKind
  /** Indent multiplier. 0 is flush with the panel's padding. */
  depth: number
  label: string
  /** Absolute work-tree root this row belongs to. Every command needs it. */
  repo: string
  /** File rows only. */
  file?: ChangeFile
  /**
   * File rows only: the id of the top-level changelist this file sits in, submodules
   * included. Commit needs it — committing "the selection" without knowing which
   * changelist it came from is how the *other* changelist gets disturbed.
   */
  changelist?: string
  /** Group rows only — what the mock right-aligns after the name. */
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
 * Row ids are built from paths, so the separator must be a character no path contains.
 * NUL is the one byte a POSIX path is guaranteed not to hold, which makes `${root}\0${p}`
 * collision-free without escaping. It never reaches the DOM as an attribute — rows are
 * keyed by it in React and looked up in Sets, nothing more.
 */
const SEP = '\u0000'

export function repoRowId(root: string): string {
  return `${root}${SEP}repo`
}

export function groupRowId(root: string, groupPath: readonly string[]): string {
  return `${root}${SEP}g${SEP}${groupPath.join(SEP)}`
}

/** The id of a file row, and the key under which it is selected. */
export function fileRowId(root: string, path: string): string {
  return `${root}${SEP}f${SEP}${path}`
}

/**
 * Flatten one `git_status` payload into rows, honouring collapsed groups.
 *
 * The repo level is elided when there is exactly one repo (§5.3): with a single root the
 * panel must look *exactly* like the mock, which has no repo header, and a header that
 * appears the moment someone adds a second root is the intended behaviour rather than a
 * layout that is always one level deeper than the design.
 */
export function buildRows(tree: ChangesTree, expanded: ReadonlySet<string>): Row[] {
  const rows: Row[] = []
  const multi = tree.repos.length > 1

  for (const repo of tree.repos) {
    if (multi) {
      const id = repoRowId(repo.root)
      // Pushed before its children exist, then patched below: the repo's descendant file
      // set is the union of its groups', which is only known after they have been walked.
      const row: Row = {
        id,
        kind: 'repo',
        depth: 0,
        label: repo.label,
        repo: repo.root,
        expandable: true,
        files: [],
      }
      rows.push(row)
      if (!expanded.has(id)) {
        // A collapsed repo still owns its files — the header's checkbox must show their
        // state, and ticking it must reach them, even though no row for them exists.
        row.files = allFileIds(repo)
        row.count = row.files.length
        continue
      }
      const start = rows.length
      for (const group of repo.groups) walkGroup(rows, repo.root, group, [group.id], 1, expanded)
      row.files = rows.slice(start).flatMap((r) => (r.kind === 'file' ? [r.id] : []))
      row.count = countFiles(repo.groups)
    } else {
      for (const group of repo.groups) walkGroup(rows, repo.root, group, [group.id], 0, expanded)
    }
  }
  return rows
}

function walkGroup(
  rows: Row[],
  root: string,
  group: ChangeGroup,
  path: string[],
  depth: number,
  expanded: ReadonlySet<string>,
): void {
  const id = groupRowId(root, path)
  const row: Row = {
    id,
    kind: 'group',
    depth,
    label: group.name,
    repo: root,
    expandable: true,
    count: countGroup(group),
    ...(group.active === true ? { active: true } : {}),
    files: [],
  }
  rows.push(row)

  if (!expanded.has(id)) {
    row.files = groupFileIds(root, group)
    return
  }

  const start = rows.length
  // Submodules before files: a submodule's changes are a group, and burying it under a
  // long file list is how a submodule pointer gets committed without being read.
  for (const child of group.groups ?? []) {
    walkGroup(rows, root, child, [...path, child.id], depth + 1, expanded)
  }
  for (const file of group.files) {
    rows.push({
      id: fileRowId(root, file.path),
      kind: 'file',
      depth: depth + 1,
      label: file.path,
      repo: root,
      file,
      // `path[0]`, not `group.id`: a file inside a submodule group belongs to the
      // changelist that submodule hangs from, and that is what commit has to name.
      changelist: path[0] ?? group.id,
      expandable: false,
      files: [fileRowId(root, file.path)],
    })
  }
  row.files = rows.slice(start).flatMap((r) => (r.kind === 'file' ? [r.id] : []))
}

function groupFileIds(root: string, group: ChangeGroup): string[] {
  const own = group.files.map((f) => fileRowId(root, f.path))
  const nested = (group.groups ?? []).flatMap((g) => groupFileIds(root, g))
  return [...nested, ...own]
}

function allFileIds(repo: RepoChanges): string[] {
  return repo.groups.flatMap((g) => groupFileIds(repo.root, g))
}

/** Files at or below a group, including its submodules. The number the mock right-aligns. */
export function countGroup(group: ChangeGroup): number {
  return group.files.length + (group.groups ?? []).reduce((n, g) => n + countGroup(g), 0)
}

function countFiles(groups: readonly ChangeGroup[]): number {
  return groups.reduce((n, g) => n + countGroup(g), 0)
}

/**
 * The tri-state of one row.
 *
 * A file is `partial` when it is selected but only some of its hunks are, and that
 * partiality has to climb: a group whose only selected file is half-staged is not a group
 * that is fully checked, and drawing it as `✓` would tell the user the commit contains
 * more than it does.
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
  repo: string
  /** Top-level changelist id, submodule files included. */
  changelist: string
  file: ChangeFile
}

/**
 * Every file in the tree, regardless of what is collapsed.
 *
 * This, and not the row list, is what commit and the footer count read. Rows only exist
 * for expanded groups, so counting them would drop the ticks inside a collapsed changelist
 * — a commit that silently omits files because a group happened to be shut is the worst
 * bug this panel could have.
 */
export function flatFiles(tree: ChangesTree): FileEntry[] {
  const out: FileEntry[] = []
  for (const repo of tree.repos) {
    const walk = (group: ChangeGroup, changelist: string) => {
      for (const g of group.groups ?? []) walk(g, changelist)
      for (const file of group.files) {
        out.push({ id: fileRowId(repo.root, file.path), repo: repo.root, changelist, file })
      }
    }
    for (const g of repo.groups) walk(g, g.id)
  }
  return out
}

/** Every file id in the tree, in row order — what "select all" and a first load use. */
export function allFiles(tree: ChangesTree): string[] {
  return flatFiles(tree).map((e) => e.id)
}

/** The ids whose file is only partly staged. The input to `checkState`'s `partial`. */
export function partialFiles(tree: ChangesTree): Set<string> {
  return new Set(flatFiles(tree).flatMap((e) => (e.file.partial === true ? [e.id] : [])))
}

/** Every collapsible row id: the repo rows and every group at any depth. "Expand all". */
export function allGroups(tree: ChangesTree): Set<string> {
  const out = new Set<string>()
  for (const repo of tree.repos) {
    out.add(repoRowId(repo.root))
    const walk = (group: ChangeGroup, path: string[]) => {
      out.add(groupRowId(repo.root, path))
      for (const g of group.groups ?? []) walk(g, [...path, g.id])
    }
    for (const g of repo.groups) walk(g, [g.id])
  }
  return out
}

/**
 * Does this row id belong to that work tree?
 *
 * The separator matters: a plain `startsWith(root)` would put `/w/app-ui`'s rows inside
 * `/w/app`, which in a multi-repo workspace means the guard bar's "reload" clearing the
 * wrong repo's ticks.
 */
export function inRepo(id: string, root: string): boolean {
  return id.startsWith(`${root}${SEP}`)
}

/**
 * What is ticked when the panel first paints.
 *
 * The active changelist and nothing else — changelists are the truth here (ADR 0004), and
 * the acceptance test is "commit one changelist, the other is untouched". Ticking every
 * changelist by default would make that a thing the user has to undo before every commit,
 * which is the point at which people stop reading the tree.
 *
 * Unversioned and ignored files are never in it: `git commit` should not sweep in
 * `target/`, and IDEA's Unversioned group is unticked for the same reason. With no group
 * marked active — a backend that does not track one yet — every changelist is ticked,
 * because the alternative is a panel that opens with nothing to commit and no clue why.
 */
export function defaultSelection(tree: ChangesTree): Set<string> {
  const out = new Set<string>()
  for (const repo of tree.repos) {
    const hasActive = repo.groups.some((g) => g.active === true)
    const walk = (group: ChangeGroup, inActive: boolean) => {
      const committable = group.kind !== 'unversioned' && group.kind !== 'ignored'
      const take = committable && (inActive || !hasActive)
      if (take) for (const f of group.files) out.add(fileRowId(repo.root, f.path))
      // A submodule group inherits its parent's fate: it is part of that changelist.
      for (const g of group.groups ?? []) walk(g, inActive)
    }
    for (const g of repo.groups) walk(g, g.active === true)
  }
  return out
}

/** Groups that start open. Ignored files are noise until asked for, exactly as in IDEA. */
export function defaultExpanded(tree: ChangesTree): Set<string> {
  const out = new Set<string>()
  for (const repo of tree.repos) {
    out.add(repoRowId(repo.root))
    const walk = (group: ChangeGroup, path: string[]) => {
      if (group.kind !== 'ignored') out.add(groupRowId(repo.root, path))
      for (const g of group.groups ?? []) walk(g, [...path, g.id])
    }
    for (const g of repo.groups) walk(g, [g.id])
  }
  return out
}

/** The order the footer counts statuses in, most-common first so the common line is short. */
const SUMMARY_ORDER: readonly FileStatus[] = [
  'modified',
  'added',
  'deleted',
  'renamed',
  'conflicted',
  'unversioned',
  'ignored',
]

/**
 * The footer's `2 modified`.
 *
 * The words are git's adjectives, so they do not pluralise — `2 modified`, not
 * `2 modifieds`. With more than one status present the parts join with a middle dot, which
 * is the separator the status bar already uses.
 */
export function summarize(files: readonly ChangeFile[]): string {
  const counts = new Map<FileStatus, number>()
  for (const f of files) counts.set(f.status, (counts.get(f.status) ?? 0) + 1)
  const parts = SUMMARY_ORDER.flatMap((s) => {
    const n = counts.get(s)
    return n === undefined || n === 0 ? [] : [`${n} ${s}`]
  })
  return parts.length === 0 ? 'nothing selected' : parts.join(' · ')
}

/** The `ChangeFile` behind each ticked id — collapsed groups included. */
export function selectedFiles(tree: ChangesTree, selected: ReadonlySet<string>): ChangeFile[] {
  return flatFiles(tree).flatMap((e) => (selected.has(e.id) ? [e.file] : []))
}

/** One repo's share of a commit. */
export interface CommitUnit {
  repo: string
  paths: string[]
  /** The changelist to commit, or `null` when the ticks span more than one. */
  changelist: string | null
}

/**
 * Split the ticked files by repo, naming the changelist when they all came from one.
 *
 * A multi-repo commit is separate commits, one per repo — git has no other kind. `null`
 * for a mixed selection is deliberate: the backend then commits exactly the paths given
 * and leaves every changelist's membership alone, which is the only honest answer to
 * "commit these files, which came from two changelists".
 */
export function commitUnits(tree: ChangesTree, selected: ReadonlySet<string>): CommitUnit[] {
  const byRepo = new Map<string, { paths: string[]; lists: Set<string> }>()
  for (const entry of flatFiles(tree)) {
    if (!selected.has(entry.id)) continue
    const unit = byRepo.get(entry.repo) ?? { paths: [], lists: new Set<string>() }
    unit.paths.push(entry.file.path)
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

// --- normalisation ----------------------------------------------------------------------

const STATUSES: readonly string[] = [
  'modified',
  'added',
  'deleted',
  'renamed',
  'unversioned',
  'ignored',
  'conflicted',
]

const GROUP_KINDS: readonly string[] = ['changelist', 'unversioned', 'ignored', 'submodule']

/**
 * Turn whatever `git_status` actually returned into a `ChangesTree`.
 *
 * The panel's single trust boundary. `git_status` is not registered yet and its DTO is
 * being written by another pair of hands, so this deliberately validates rather than
 * casts: anything unrecognised is dropped, and the worst outcome is an empty panel. The
 * alternative — `raw as ChangesTree` — turns a field rename into a TypeError inside
 * render, which in React 19 unmounts the whole window rather than the panel.
 *
 * It is also the only reason the panel can ship before the backend does.
 */
export function normalizeStatus(raw: unknown): ChangesTree {
  if (!isRecord(raw) || !Array.isArray(raw['repos'])) return { repos: [] }
  const repos = raw['repos'].flatMap((r) => {
    const repo = normalizeRepo(r)
    return repo ? [repo] : []
  })
  return { repos }
}

function normalizeRepo(raw: unknown): RepoChanges | null {
  if (!isRecord(raw)) return null
  const root = str(raw['root'])
  if (root === undefined) return null
  const groups = Array.isArray(raw['groups'])
    ? raw['groups'].flatMap((g, i) => {
        const group = normalizeGroup(g, i)
        return group ? [group] : []
      })
    : []
  return {
    root,
    // A root with no label is still a usable row; the last path segment is what the file
    // tree shows for the same directory.
    label: str(raw['label']) ?? root.split('/').filter(Boolean).pop() ?? root,
    ...optional('branch', str(raw['branch'])),
    ...optional('headMessage', str(raw['headMessage'])),
    ...optional('indexDiverged', bool(raw['indexDiverged'])),
    ...optional('indexToken', str(raw['indexToken'])),
    groups,
  }
}

function normalizeGroup(raw: unknown, index: number): ChangeGroup | null {
  if (!isRecord(raw)) return null
  const name = str(raw['name'])
  if (name === undefined) return null
  const kindRaw = str(raw['kind'])
  const kind = (kindRaw !== undefined && GROUP_KINDS.includes(kindRaw)
    ? kindRaw
    : 'changelist') as ChangeGroup['kind']
  const files = Array.isArray(raw['files'])
    ? raw['files'].flatMap((f) => {
        const file = normalizeFile(f)
        return file ? [file] : []
      })
    : []
  const nested = Array.isArray(raw['groups'])
    ? raw['groups'].flatMap((g, i) => {
        const group = normalizeGroup(g, i)
        return group ? [group] : []
      })
    : []
  return {
    // A backend that omits ids still needs row ids that are unique and stable within one
    // payload; position plus name is both, and only breaks if two sibling groups share a
    // name, which the domain forbids.
    id: str(raw['id']) ?? `${index}:${name}`,
    name,
    kind,
    files,
    ...(nested.length > 0 ? { groups: nested } : {}),
    ...optional('active', bool(raw['active'])),
  }
}

function normalizeFile(raw: unknown): ChangeFile | null {
  if (!isRecord(raw)) return null
  const path = str(raw['path'])
  if (path === undefined) return null
  const statusRaw = str(raw['status'])
  return {
    path,
    // An unknown status is shown as `modified` rather than dropped: the file is genuinely
    // changed, and hiding a row would understate what a commit is about to include.
    status: (statusRaw !== undefined && STATUSES.includes(statusRaw)
      ? statusRaw
      : 'modified') as FileStatus,
    ...optional('originalPath', str(raw['originalPath'])),
    ...optional('partial', bool(raw['partial'])),
  }
}

/**
 * Spread-in for an optional property under `exactOptionalPropertyTypes`.
 *
 * `{ branch: undefined }` is a type error against `branch?: string` with that flag on, and
 * writing the conditional spread inline at every field turns each one into three lines.
 */
function optional<K extends string, V>(key: K, value: V | undefined): { [P in K]?: V } {
  return (value === undefined ? {} : { [key]: value }) as { [P in K]?: V }
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
