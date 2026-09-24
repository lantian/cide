import type { Bootstrap, Geometry, SessionExit, TreeStatusMap } from '../ipc/generated'
import type { Args, Handler } from './fakeTauri'
import type { SceneId } from './sceneIds'
import { encode, screen } from './transcripts'
import { HOME, makeWorld, sessions, TREE_STATUS, treeRows, wire, type World } from './world'
import { SCENES } from './sceneTable'
import { BRANCHES, STATUS } from './data/git'

/**
 * `cide-headless demo-bootstrap`'s output, written here by `demo-shots.mjs` before every capture
 * and gitignored. Reached through `import.meta.glob` rather than a plain import on purpose: a
 * glob that matches nothing is `{}`, so `tsc` and CI — where the file never exists — stay green,
 * and the page says what to run instead of failing to resolve a module.
 */
const generated = import.meta.glob<Bootstrap>('./bootstrap.gen.json', { eager: true, import: 'default' })

export type Theme = 'dark' | 'light'

export type Handlers = Map<string, Handler>

/** What a scene adds on top of the base world. */
export interface Scene {
  /** Reshape the world and add handlers. Runs before the app boots. */
  setup: (world: World, handlers: Handlers, theme: Theme) => void
  /** Drive the booted app into the state the picture is of: open a panel, a settings page. */
  drive?: () => Promise<void>
  /** Keystrokes `demo-shots.mjs` sends as trusted input once the scene is ready. */
  keys?: string[]
}

export interface BuiltScene {
  window: string
  handlers: Handlers
  ready: () => Promise<void>
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/** The answers every scene needs: the project, its tree, its terminals. */
function baseHandlers(world: World, theme: Theme): Handlers {
  const rows = treeRows()
  const geometry = new Map<string, Geometry>()
  const draw = (session: string): ArrayBuffer => {
    const content = sessions.get(session)
    const geo = geometry.get(session) ?? { cols: 100, rows: 30, cellWidth: 8, cellHeight: 16 }
    return encode(content ? `\x1b[H\x1b[2J${screen(content, theme, geo.cols, geo.rows)}` : '')
  }
  const status = { indexing: false, files: 1843, dirs: 212, rows: rows.length, watch: { backend: 'native' as const, reason: null, watchedDirs: 212 } }
  return new Map<string, Handler>([
    ['app_get_bootstrap', () => world.boot],
    ['workspace_rev', () => world.boot.workspace.rev],
    ['ext_snapshot', () => ({ rev: wire(1), marketplaces: [], extensions: [], resolved: world.boot.extensions, problems: [] })],
    ['fs_index', () => status],
    ['fs_status', () => status],
    ['fs_tree_count', () => rows.length],
    ['fs_tree_rows', (a: Args) => rows.slice(Number(a['offset']), Number(a['offset']) + Number(a['len']))],
    ['fs_writable_roots', () => [HOME]],
    ['fs_reveal_roots', () => [HOME]],
    ['git_tree_status', (): TreeStatusMap => ({ statuses: TREE_STATUS, truncated: false })],
    ['git_status', () => STATUS],
    ['git_branch_list', () => BRANCHES],
    ['session_list', () => [...sessions.keys()]],
    ['session_exit', (): SessionExit => ({ kind: 'running' })],
    ['session_resumable', () => true],
    ['session_attach', (a: Args) => {
      const session = String(a['session'])
      geometry.set(session, a['geometry'] as Geometry)
      return draw(session)
    }],
    ['session_resize', (a: Args) => {
      geometry.set(String(a['session']), a['geometry'] as Geometry)
      return null
    }],
    ['session_scrollback', (a: Args) => draw(String(a['session']))],
  ])
}

export function buildScene(id: string, theme: Theme): BuiltScene {
  const seed = generated['./bootstrap.gen.json']
  if (!seed) {
    document.body.textContent =
      'src/demo/bootstrap.gen.json is missing — run `pnpm --dir ui demo:shots`, or '
      + '`./target/debug/cide-headless demo-bootstrap > ui/src/demo/bootstrap.gen.json`.'
    throw new Error('demo bootstrap missing')
  }
  const world = makeWorld(seed)
  world.boot.workspace.settings.theme = theme
  const handlers = baseHandlers(world, theme)
  const scene = SCENES[id as SceneId]
  if (!scene) throw new Error(`no such demo scene: ${id}`)
  scene.setup(world, handlers, theme)
  return {
    window: world.boot.window,
    handlers,
    ready: async () => {
      // Let the boot's own round trips land — bootstrap, the tree, each pane's attach — before
      // driving anything; a panel requested before the sidebar host mounts is a request nobody
      // answers.
      await sleep(1200)
      if (scene.drive) await scene.drive()
      ;(window as unknown as { __demoKeys?: string[] }).__demoKeys = scene.keys ?? []
      await sleep(600)
      await document.fonts.ready
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
      document.documentElement.dataset['demoReady'] = '1'
    },
  }
}
