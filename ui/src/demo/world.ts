/**
 * The demo's world: one project called `cide`, open on its own source, with the shape a real
 * afternoon of work has — a Claude grid, a dirty file, a few changes in git.
 *
 * Built on top of `cide-headless demo-bootstrap` rather than from nothing, so the settings,
 * keymap, command registry and language table are the Rust core's own; only the *project* is
 * reshaped here, because the Rust demo exists to exercise the layout code (three nested splits, a
 * diff pane with no diff behind it) and a screenshot wants a layout a person would choose.
 */
import type {
  Bootstrap,
  Pane,
  PaneId,
  PaneKind,
  Project,
  SessionId,
  Tab,
  TabId,
  TabKind,
  TreeRow,
  TreeStatus,
} from '../ipc/generated'
import type { TerminalContent } from './transcripts'

export const HOME = '/home/dev/work/cide'

let counter = 0
/** Deterministic ids: a screenshot that changes because a uuid did is noise in a diff of PNGs. */
export function uid(prefix: string): string {
  counter++
  const hex = counter.toString(16).padStart(12, '0')
  return `${prefix.padEnd(8, '0').slice(0, 8)}-0000-4000-8000-${hex}`
}

/** ts-rs renders `u64`/`i64` as `bigint`; the wire delivers JSON numbers. So does the demo. */
export const wire = (n: number): bigint => n as unknown as bigint

type Node = Tab['tree']['root']

export function leaf(pane: PaneId): Node {
  return { kind: 'leaf', pane }
}

export function split(axis: 'row' | 'col', a: Node, b: Node, ratio = 0.5): Node {
  return { kind: 'split', id: uid('split'), axis, a, b, ratio }
}

/** Every terminal session the world holds, and what it should show. */
export const sessions = new Map<SessionId, TerminalContent>()

export function pane(kind: PaneKind, title: string, content?: TerminalContent, primary = false): Pane {
  const session = content ? uid('session') : null
  if (session && content) sessions.set(session, content)
  return {
    id: uid('pane'),
    kind,
    role: primary ? 'primary' : 'auxiliary',
    session,
    conversation: null,
    conversationSince: null,
    continues: null,
    title,
  }
}

export function tab(kind: TabKind, root: Node, panes: Pane[], focused?: PaneId): Tab {
  return {
    id: uid('tab'),
    kind,
    tree: {
      root,
      focused: focused ?? panes[0]?.id ?? '',
      maximized: null,
      panes: Object.fromEntries(panes.map((p) => [p.id, p])),
    },
  }
}

export function fileTab(path: string, dirty = false): Tab {
  const editor = pane('editor', path.split('/').pop() ?? path)
  return tab({ kind: 'file', path, dirty }, leaf(editor.id), [editor])
}

/** The 2×2 Claude grid the product is built around: three conversations and a shell. */
export function claudeHome(): { tab: Tab; primary: SessionId } {
  const main = pane('claude', 'cide : claude', { kind: 'claude', conversation: 'feature' }, true)
  const tests = pane('claude', 'cide : claude — tests', { kind: 'claude', conversation: 'tests' })
  const review = pane('claude', 'cide : claude — review', { kind: 'claude', conversation: 'review' })
  const shell = pane('shell', 'cide : bash', { kind: 'shell' })
  const root = split('col', split('row', leaf(main.id), leaf(tests.id), 0.55), split('row', leaf(review.id), leaf(shell.id), 0.55), 0.6)
  return { tab: tab({ kind: 'claudeHome' }, root, [main, tests, review, shell], main.id), primary: main.session ?? '' }
}

export interface World {
  boot: Bootstrap
  project: Project
  /** Add a tab to the demo project and make it the active one. */
  open: (t: Tab) => TabId
}

export function makeWorld(seed: Bootstrap): World {
  const boot: Bootstrap = structuredClone(seed)
  const ws = boot.workspace
  const ids = Object.keys(ws.projects)
  const original = ws.projects[ids[0] ?? '']
  if (!original) throw new Error('demo bootstrap has no project')

  const home = claudeHome()
  const project: Project = {
    ...original,
    name: 'cide',
    displayPath: '~/work/cide',
    roots: [{ path: HOME, label: 'cide' }],
    tabs: [home.tab],
    activeTab: home.tab.id,
    tabMru: [home.tab.id],
    detached: {},
    dockAnchors: {},
    primarySession: home.primary,
    toolWindow: { ...original.toolWindow, open: false, history: [], active: null },
  }
  ws.projects[project.id] = project

  // The other two projects stay, renamed to look like a working set, so the header shows three
  // project tabs — which is the multi-project story told for free — but each keeps only its
  // pinned Claude tab, with one conversation, so nothing behind them asks for more data.
  const names = ['atlas', 'docs-site']
  ids.slice(1).forEach((id, i) => {
    const other = ws.projects[id]
    if (!other) return
    const home2 = pane('claude', `${names[i]} : claude`, { kind: 'claude', conversation: 'refactor' }, true)
    const t = tab({ kind: 'claudeHome' }, leaf(home2.id), [home2])
    ws.projects[id] = {
      ...other,
      name: names[i] ?? other.name,
      displayPath: `~/work/${names[i]}`,
      roots: [{ path: `/home/dev/work/${names[i]}`, label: names[i] ?? other.name }],
      tabs: [t],
      activeTab: t.id,
      tabMru: [t.id],
      detached: {},
      dockAnchors: {},
      primarySession: home2.session ?? '',
    }
  })
  for (const role of Object.values(ws.windows)) {
    if (role.kind === 'shell') role.active = project.id
  }
  if (boot.role.kind === 'shell') boot.role.active = project.id

  return {
    boot,
    project,
    open: (t) => {
      project.tabs.push(t)
      project.activeTab = t.id
      project.tabMru = [t.id, ...project.tabMru]
      return t.id
    },
  }
}

// --- the file tree -------------------------------------------------------------------

const TREE: Array<[string, number, 'dir' | 'file', boolean?]> = [
  ['.cide', 0, 'dir'],
  ['contract', 0, 'dir'],
  ['crates', 0, 'dir', true],
  ['cide-agents', 1, 'dir'],
  ['cide-app', 1, 'dir'],
  ['cide-claude', 1, 'dir'],
  ['cide-core', 1, 'dir'],
  ['cide-git', 1, 'dir'],
  ['cide-lsp', 1, 'dir'],
  ['cide-pty', 1, 'dir', true],
  ['src', 2, 'dir', true],
  ['coalesce.rs', 3, 'file'],
  ['lib.rs', 3, 'file'],
  ['session.rs', 3, 'file'],
  ['vt.rs', 3, 'file'],
  ['Cargo.toml', 2, 'file'],
  ['cide-tasks', 1, 'dir'],
  ['docs', 0, 'dir'],
  ['packaging', 0, 'dir'],
  ['scripts', 0, 'dir'],
  ['ui', 0, 'dir'],
  ['xtask', 0, 'dir'],
  ['build.sh', 0, 'file'],
  ['Cargo.lock', 0, 'file'],
  ['Cargo.toml', 0, 'file'],
  ['CLAUDE.md', 0, 'file'],
  ['CONTRIBUTING.md', 0, 'file'],
  ['LICENSE', 0, 'file'],
  ['README.md', 0, 'file'],
  ['run.sh', 0, 'file'],
  ['rust-toolchain.toml', 0, 'file'],
]

export function treeRows(): TreeRow[] {
  const stack: string[] = [HOME]
  const rows: TreeRow[] = TREE.map(([name, depth, kind, expanded]) => {
    stack.length = depth + 1
    const path = `${stack[depth]}/${name}`
    stack[depth + 1] = path
    return {
      path,
      name,
      depth,
      kind,
      expanded: Boolean(expanded),
      hasChildren: kind === 'dir',
      symlink: false,
      root: 0,
      detail: null,
    }
  })
  const group = (name: string, kind: TreeRow['kind'], detail: string | null): TreeRow => ({
    path: `cide:${name}`,
    name,
    depth: 0,
    kind,
    expanded: false,
    hasChildren: kind === 'group',
    symlink: false,
    root: 0,
    detail,
  })
  rows.push(group('Project Notes', 'note', null), group('External Libraries', 'group', null), group('Scratches', 'group', null))
  return rows
}

export const TREE_STATUS: Record<string, TreeStatus> = {
  [`${HOME}/crates/cide-pty/src/coalesce.rs`]: 'added',
  [`${HOME}/crates/cide-pty/src/session.rs`]: 'modified',
  [`${HOME}/crates/cide-pty/src/lib.rs`]: 'modified',
  [`${HOME}/crates/cide-pty`]: 'modified',
  [`${HOME}/crates/cide-pty/src`]: 'modified',
  [`${HOME}/crates`]: 'modified',
  [`${HOME}/README.md`]: 'modified',
}
