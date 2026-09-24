import type { DockerBoard, DockerDetail, Geometry } from '../../ipc/generated'
import { emitEvent, type Args } from '../fakeTauri'
import { DOCKER_BOARD, execScreen, logsScreen, RELAY, RELAY_DETAIL } from '../data/docker'
import { click, sleep, until } from '../drive'
import type { Scene } from '../scenes'
import { encode } from '../transcripts'
import { leaf, pane, split } from '../world'

/**
 * Docker: the bottom panel's Docker tab — two compose stacks, loose containers, images, volumes
 * and networks — under a Claude tab that has grown two container panes beside the conversation:
 * an exec shell inside the remote relay and the same container's log stream.
 *
 * The Docker tab is webview state (`ToolWindowHost`'s `dockerActive`, never persisted), so the
 * world can only open the bottom panel; `drive` clicks the tab, as a user would.
 */
export const docker: Scene = {
  setup: (world, handlers) => {
    const home = world.project.tabs[0]
    const main = home && Object.values(home.tree.panes).find((p) => p.role === 'primary')
    if (!home || !main) throw new Error('docker scene: the base world has no Claude tab')

    // Container panes are `shell` panes with a `docker` field — see `cide_ipc::Pane::docker` —
    // titled after the container, as `cmd/pane.rs` names them. The `{kind: 'shell'}` content only
    // registers the session; what it draws is overridden below.
    const exec = { ...pane('shell', `cide : ${RELAY.name}`, { kind: 'shell' }), docker: { container: RELAY.id, name: RELAY.name, stream: 'exec' as const } }
    const logs = { ...pane('shell', `cide : ${RELAY.name}`, { kind: 'shell' }), docker: { container: RELAY.id, name: RELAY.name, stream: 'logs' as const } }
    home.tree = {
      root: split('row', leaf(main.id), split('col', leaf(exec.id), leaf(logs.id), 0.5), 0.5),
      focused: exec.id,
      maximized: null,
      panes: { [main.id]: main, [exec.id]: exec, [logs.id]: logs },
    }
    world.project.toolWindow = { ...world.project.toolWindow, open: true, height: 400 }

    const screens = new Map<string, (rows: number) => string>([
      [exec.session ?? '', execScreen],
      [logs.session ?? '', logsScreen],
    ])
    const rows = new Map<string, number>()
    const draw = (session: string) => encode(`\x1b[H\x1b[2J${screens.get(session)?.(rows.get(session) ?? 30) ?? ''}`)
    // Wrap the base terminal handlers: the two container sessions draw their own screens, every
    // other session (the Claude conversation) keeps the base transcript.
    const passThrough = (cmd: string, mine: (a: Args) => unknown) => {
      const base = handlers.get(cmd)
      handlers.set(cmd, (a: Args) => (screens.has(String(a['session'])) ? mine(a) : base?.(a)))
    }
    passThrough('session_attach', (a) => {
      const session = String(a['session'])
      rows.set(session, (a['geometry'] as Geometry).rows)
      return draw(session)
    })
    passThrough('session_resize', (a) => {
      rows.set(String(a['session']), (a['geometry'] as Geometry).rows)
      return null
    })
    passThrough('session_scrollback', (a) => draw(String(a['session'])))

    handlers.set('docker_board', (): DockerBoard => DOCKER_BOARD)
    handlers.set('docker_detail', (): DockerDetail => RELAY_DETAIL)
  },
  drive: async () => {
    const tab = () =>
      [...document.querySelectorAll<HTMLButtonElement>('[data-audit="toolWindowTab"] button[role="tab"]')].find(
        (b) => b.textContent?.trim() === 'Docker',
      )
    await until(() => tab() !== undefined)
    // Hand the store its board *before* the panel mounts, as the daemon watch would have. The
    // panel's `Body` calls hooks after its `unknown` early return, so mounting on an unread board
    // and then reading one trips React's hook-order check (an app bug, reported, not fixed here).
    emitEvent('cide://docker-changed', { board: DOCKER_BOARD })
    await sleep(100)
    tab()?.click()
    await sleep(400)
    // Select the relay, so the detail pane beside the list has something to say.
    await click(`[data-audit="dockerContainer"]:has([title^="${RELAY.name} "])`)
    await sleep(400)
  },
}
