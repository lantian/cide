/**
 * The `log` scene's history: `pty-backpressure` on top of a master that merges its feature
 * branches, which is what gives the graph its lanes.
 *
 * Built from the generated wire types, as `gitlog/fixture.ts` is. That fixture's six rows are
 * shaped to pin edge cases (a forty-tag commit, a year boundary), not to look like an afternoon
 * in a busy repository, so the demo writes its own history and lays out the gutter with a small
 * lane walk below rather than by hand — thirty rows of hand-written edges is thirty chances to
 * draw a line to nowhere.
 */
import type {
  CommitDetail,
  CommitFile,
  CommitPage,
  CommitRow,
  GraphEdge,
  GraphRow,
  RefChip,
} from '../../ipc/generated'
import { wire } from '../world'
import { REPO } from './git'

/** "Now", matching the base world's branch list. */
const NOW = 1_790_000_000
const HOUR = 3600

/** A stable forty-hex oid per stem, so `shortOid` really is a prefix of `oid`. */
function oid(stem: string): string {
  let h = 0x811c9dc5
  let out = stem
  while (out.length < 40) {
    for (const c of out) h = Math.imul(h ^ c.charCodeAt(0), 16777619) >>> 0
    out += h.toString(16).padStart(8, '0')
  }
  return out.slice(0, 40)
}

const chip = (kind: RefChip['kind'], name: string, full: string, current = false): RefChip => ({ kind, name, full, current })

type Spec = {
  stem: string
  summary: string
  author: string
  hoursAgo: number
  parents: string[]
  refs?: RefChip[]
}

/*
 * Newest first, as the panel draws it. Stems are real-looking seven-hex prefixes; parents name
 * stems. Two feature branches were merged into master (`perf-workspace`, `mr-review`), and the
 * current branch sits two commits ahead of its upstream.
 */
const SPECS: Spec[] = [
  { stem: 'a41c9e0', summary: 'Hold frames past the ack window', author: 'dev', hoursAgo: 1, parents: ['7d20b13'],
    refs: [chip('head', 'HEAD', 'HEAD', true), chip('localBranch', 'pty-backpressure', 'refs/heads/pty-backpressure', true)] },
  { stem: '7d20b13', summary: 'Cover backpressure with a paused-reader test', author: 'dev', hoursAgo: 2, parents: ['c58e4f2'] },
  { stem: 'c58e4f2', summary: 'Split the coalescer out of session.rs', author: 'dev', hoursAgo: 4, parents: ['f921463'],
    refs: [chip('remoteBranch', 'origin/pty-backpressure', 'refs/remotes/origin/pty-backpressure')] },
  { stem: 'f921463', summary: 'Journal: small freezes, found by reading (M102)', author: 'mira', hoursAgo: 20, parents: ['e03b7a1'],
    refs: [chip('localBranch', 'master', 'refs/heads/master'), chip('remoteBranch', 'origin/master', 'refs/remotes/origin/master')] },
  { stem: 'e03b7a1', summary: 'Merge branch ’perf-workspace’', author: 'mira', hoursAgo: 21, parents: ['d85b1a0', 'a12cb36'] },
  { stem: 'd85b1a0', summary: 'Refit off-screen panes after a resize settles; cache diff sides', author: 'mira', hoursAgo: 22, parents: ['2784173'] },
  { stem: 'a12cb36', summary: 'Share unchanged workspace subtrees and memoise panes', author: 'alex', hoursAgo: 23, parents: ['2f1a204'] },
  { stem: '2f1a204', summary: 'Stop re-rendering for unchanged rosters, stores and log rows', author: 'alex', hoursAgo: 26, parents: ['2784173'] },
  { stem: '2784173', summary: 'Keep language-server and project-close teardown off the main thread', author: 'mira', hoursAgo: 30, parents: ['9b3d5e8'] },
  { stem: '9b3d5e8', summary: 'Merge branch ’mr-review’', author: 'dev', hoursAgo: 44, parents: ['c26ba62', 'd10a932'],
    refs: [chip('tag', 'v0.10.0', 'refs/tags/v0.10.0')] },
  { stem: 'd10a932', summary: 'MR review: hunks, Commits, a comment walk, Discuss on a draft (M101)', author: 'sam', hoursAgo: 46, parents: ['4e8f0a6'],
    refs: [chip('localBranch', 'gitlab-review', 'refs/heads/gitlab-review')] },
  { stem: 'c26ba62', summary: 'The app on the UI kit, an unfinished milestone’s inbox (M99-M100)', author: 'dev', hoursAgo: 50, parents: ['6a31e0f'] },
  { stem: '4e8f0a6', summary: 'Draft comments from an agent review survive a restart', author: 'sam', hoursAgo: 55, parents: ['1c7a2d4'] },
  { stem: '6a31e0f', summary: 'Codex as a console, busy chips, the New project wizard (M93-M98)', author: 'dev', hoursAgo: 70, parents: ['65cef1b'] },
  { stem: '1c7a2d4', summary: 'GitLab: read a merge request’s discussions page by page', author: 'sam', hoursAgo: 74, parents: ['65cef1b'] },
  { stem: '65cef1b', summary: 'Isolated directories per worktree, an honest verify refusal (M92)', author: 'alex', hoursAgo: 96, parents: ['f2b5e64'] },
  { stem: 'f2b5e64', summary: 'Milestones and gates, agent MR review, pool limits, the phone (M83-M91)', author: 'dev', hoursAgo: 120, parents: ['ad4ee8b'],
    refs: [chip('tag', 'v0.9.2', 'refs/tags/v0.9.2')] },
  { stem: 'ad4ee8b', summary: 'Merge branch ’master’ of github.com:dev/cide', author: 'dev', hoursAgo: 140, parents: ['b59aad3', '2fa5cc9'] },
  { stem: 'b59aad3', summary: 'A reviewer that opens itself, who ran a line, MiMo, and auto (M79-M82)', author: 'dev', hoursAgo: 150, parents: ['e7cdb83'] },
  { stem: '2fa5cc9', summary: 'GitLab integration: review a merge request in cide', author: 'alex', hoursAgo: 160, parents: ['e7cdb83'] },
  { stem: 'e7cdb83', summary: 'Starting status fix for an overridden harness', author: 'mira', hoursAgo: 170, parents: ['6222426'] },
  { stem: '6222426', summary: 'A phone can reach cide, two settings that reached nothing (M72-M77)', author: 'dev', hoursAgo: 200, parents: ['ee9ac14'] },
  { stem: 'ee9ac14', summary: 'A graceful stop, a task as a file, and what a file is (M65-M71)', author: 'dev', hoursAgo: 240, parents: ['40fa684'] },
  { stem: '40fa684', summary: 'Godot’s server, quick documentation, task links and drawings (M59-M64)', author: 'mira', hoursAgo: 290, parents: ['e615735'] },
  { stem: 'e615735', summary: 'An extension’s server supersedes a builtin’s; the render test waits on its sink', author: 'alex', hoursAgo: 310, parents: ['565fd9b'] },
  { stem: '565fd9b', summary: 'Resume with new harness params uses the new params', author: 'dev', hoursAgo: 330, parents: ['3fe8eac'] },
  { stem: '3fe8eac', summary: 'The dead-session attach waits for the mirror, not just the exit', author: 'mira', hoursAgo: 360, parents: ['761967e'] },
  { stem: '761967e', summary: 'The rebase differential compares the tree, not the tip’s oid', author: 'alex', hoursAgo: 380, parents: ['ad877b5'] },
  { stem: 'ad877b5', summary: 'Socket probing takes the machine’s root as a parameter', author: 'dev', hoursAgo: 400, parents: ['3ae2f45'],
    refs: [chip('tag', 'v0.9.1', 'refs/tags/v0.9.1')] },
  { stem: '3ae2f45', summary: 'Lockfile catches up with 0.9.1-dev', author: 'dev', hoursAgo: 420, parents: ['5bd196d'] },
  { stem: '5bd196d', summary: 'Back to development: 0.9.1-dev', author: 'dev', hoursAgo: 430, parents: ['14bc89b'] },
  { stem: '14bc89b', summary: '«Open Project» command', author: 'mira', hoursAgo: 450, parents: ['523c996'] },
  { stem: '523c996', summary: 'Keep run.sh on the bash macOS actually ships', author: 'alex', hoursAgo: 470, parents: ['c0f3235'] },
  { stem: 'c0f3235', summary: 'Docker support', author: 'sam', hoursAgo: 500, parents: ['7b3fa64'] },
]

const byStem = new Map(SPECS.map((s) => [s.stem, s]))

export const LOG_ROWS: CommitRow[] = SPECS.map((s) => {
  const t = wire(NOW - s.hoursAgo * HOUR)
  return {
    repo: REPO.id,
    oid: oid(s.stem),
    shortOid: s.stem,
    summary: s.summary,
    author: s.author,
    authorEmail: `${s.author}@users.noreply.example.com`,
    authored: t,
    committed: t,
    parents: s.parents.map(oid),
    pruned: 0,
    refs: s.refs ?? [],
  }
})

/**
 * The gutter, by the rule `cide_git::lanes` follows: a commit takes the leftmost lane waiting
 * for it, its first parent inherits that lane, and every further parent gets a lane of its own
 * unless one is already waiting for it. Lanes keep their column until they close, so a line
 * never jogs sideways while it passes other rows.
 */
function layout(rows: Spec[]): { rows: GraphRow[]; lanes: number } {
  type Lane = { awaits: string; color: number } | null
  const lanes: Lane[] = []
  let nextColor = 0
  let width = 0
  const free = (avoid: number): number => {
    const i = lanes.findIndex((l, k) => l === null && k !== avoid)
    return i >= 0 ? i : lanes.length
  }
  const out: GraphRow[] = rows.map((spec) => {
    const waiting = lanes.flatMap((l, i) => (l && l.awaits === spec.stem ? [i] : []))
    const lane = waiting[0] ?? free(-1)
    const color = waiting.length > 0 ? lanes[lane]!.color : nextColor++
    const edges: GraphEdge[] = []
    lanes.forEach((l, i) => {
      if (l === null) return
      if (waiting.includes(i)) edges.push({ kind: 'enter', top: i, color: l.color })
      else edges.push({ kind: 'pass', lane: i, color: l.color })
    })
    for (const i of waiting) lanes[i] = null
    spec.parents.forEach((parent, p) => {
      const known = byStem.has(parent)
      const existing = lanes.findIndex((l) => l !== null && l.awaits === parent)
      if (existing >= 0) {
        edges.push({ kind: 'exit', bottom: existing, color: lanes[existing]!.color, dangling: false })
        return
      }
      const target = p === 0 ? lane : free(lane)
      const c = p === 0 ? color : nextColor++
      lanes[target] = { awaits: parent, color: c }
      edges.push({ kind: 'exit', bottom: target, color: c, dangling: !known })
    })
    width = Math.max(width, lanes.length, lane + 1)
    return { lane, color, edges, overflow: false }
  })
  return { rows: out, lanes: width }
}

const GRAPH = layout(SPECS)

export const LOG_PAGE: CommitPage = {
  commits: LOG_ROWS,
  graph: { kind: 'rows', rows: GRAPH.rows, lanes: GRAPH.lanes, overflow: false },
  resume: null,
  repos: [{ repo: REPO.id, name: REPO.name, stop: 'exhausted', scanned: LOG_ROWS.length }],
  stop: 'exhausted',
  scanned: LOG_ROWS.length,
  cancelled: false,
  renames: [],
  followed: false,
  followCapped: false,
}

/** The commit the scene selects: the one that split the coalescer out. */
export const SELECTED = oid('c58e4f2')

const file = (path: string, added: number, deleted: number, status: CommitFile['status'] = 'modified'): CommitFile => ({
  path,
  oldPath: null,
  status,
  binary: false,
  lines: { kind: 'counted', added, deleted },
})

const DETAIL_FILES: CommitFile[] = [
  file('crates/cide-pty/src/coalesce.rs', 142, 0, 'added'),
  file('crates/cide-pty/src/session.rs', 21, 38),
  file('crates/cide-pty/src/lib.rs', 3, 0),
  file('crates/cide-pty/src/reader.rs', 17, 4),
  file('crates/cide-pty/Cargo.toml', 1, 0),
  file('crates/cide-app/src/cmd/session.rs', 12, 2),
  file('ui/src/panes/sessionSink.ts', 9, 3),
  file('docs/architecture.md', 14, 5),
]

/** `git_commit_detail` for any row: the selected one is written out, the rest are summaries. */
export function detailFor(rev: string): CommitDetail {
  const row = LOG_ROWS.find((r) => r.oid === rev || r.shortOid === rev) ?? LOG_ROWS[2]!
  const spec = byStem.get(row.shortOid)
  const selected = row.oid === SELECTED
  const files = selected ? DETAIL_FILES : [file('crates/cide-core/src/workspace.rs', 18, 6), file('ui/src/store/workspace.ts', 7, 2)]
  const total = files.reduce(
    (t, f) => (f.lines.kind === 'counted' ? { ...t, added: t.added + f.lines.added, deleted: t.deleted + f.lines.deleted } : t),
    { files: files.length, added: 0, deleted: 0, partial: false },
  )
  const message = selected
    ? 'Split the coalescer out of session.rs\n\n' +
      'The session owned three things at once: the mirror, the sinks and the frame under\n' +
      'construction. The last one is the part with a policy — when to flush, how far the\n' +
      'webview may fall behind — and it was untestable where it lived, because testing it\n' +
      'meant spawning a child.\n\n' +
      'coalesce.rs now holds it behind a pure push/take_due/ack interface, and session.rs\n' +
      'is back to fanning bytes out. No behaviour change; the backpressure itself is the\n' +
      'next commit.\n'
    : `${row.summary}\n`
  return {
    commit: row,
    message,
    committer: row.author,
    committerEmail: row.authorEmail,
    against: spec && spec.parents[0] ? { kind: 'parent', index: 0, oid: oid(spec.parents[0]) } : { kind: 'emptyTree' },
    files,
    filesTruncated: false,
    merge: row.parents.length > 1,
    total,
  }
}
