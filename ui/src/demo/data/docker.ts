/**
 * The Docker scene's wire data: the daemon on the machine cide is developed on.
 *
 * Two compose stacks a cide developer would plausibly keep — the relay the remote surface talks
 * to, and a throwaway GitLab the MR-review work is tested against — plus the loose containers
 * that pile up beside them. Wire shapes (`DockerBoard`), not the panel's `Board`: the scene
 * answers `docker_board` and `DockerPanel/adapt.ts` does the translation, so a field renamed in
 * Rust fails `tsc` here. The shape follows `sidebar/DockerPanel/fixture.ts`.
 */
import type { ContainerRow, DockerBoard, DockerDetail, ImageRow } from '../../ipc/generated'
import { HOME, wire } from '../world'

/** A stable 64-hex id from a short seed; a screenshot that changes because an id did is noise. */
export function hexId(seed: string): string {
  let h = 0x811c9dc5
  let out = ''
  while (out.length < 64) {
    for (const c of seed + out.length) h = Math.imul(h ^ c.charCodeAt(0), 0x01000193) >>> 0
    out += h.toString(16).padStart(8, '0')
  }
  return out.slice(0, 64)
}

/** Seconds since the epoch, `ago` seconds before the demo's afternoon. */
const NOW = 1_790_000_000
const at = (ago: number) => wire(NOW - ago)

const REMOTE = `${HOME}/packaging/remote/compose.yaml`
const GITLAB = `${HOME}/scripts/gitlab-sandbox/compose.yaml`

function row(over: Partial<ContainerRow> & Pick<ContainerRow, 'name' | 'image'>): ContainerRow {
  return {
    id: hexId(over.name),
    state: 'running',
    status: 'Up 3 hours',
    created: at(3 * 3600),
    ports: [],
    ...over,
  }
}

export const RELAY = row({
  name: 'cide-remote-relay-1',
  image: 'cide-relay:dev',
  status: 'Up 42 minutes (healthy)',
  health: 'healthy',
  created: at(42 * 60),
  ports: [{ private: 8443, public: 8443, protocol: 'tcp', hostIp: '127.0.0.1' }],
  compose: { project: 'cide-remote', service: 'relay', workingDir: `${HOME}/packaging/remote`, configFiles: [REMOTE] },
})

const CONTAINERS: ContainerRow[] = [
  RELAY,
  row({
    name: 'cide-remote-redis-1',
    image: 'redis:7.4-alpine',
    ports: [{ private: 6379, protocol: 'tcp' }],
    compose: { project: 'cide-remote', service: 'redis', workingDir: `${HOME}/packaging/remote`, configFiles: [REMOTE] },
  }),
  row({
    name: 'cide-remote-caddy-1',
    image: 'caddy:2.8',
    ports: [
      { private: 443, public: 443, protocol: 'tcp', hostIp: '0.0.0.0' },
      { private: 80, public: 80, protocol: 'tcp', hostIp: '0.0.0.0' },
    ],
    compose: { project: 'cide-remote', service: 'caddy', workingDir: `${HOME}/packaging/remote`, configFiles: [REMOTE] },
  }),
  row({
    name: 'gitlab-sandbox-gitlab-1',
    image: 'gitlab/gitlab-ce:17.3.1-ce.0',
    status: 'Up 4 minutes (health: starting)',
    health: 'starting',
    created: at(26 * 3600),
    ports: [
      { private: 80, public: 8929, protocol: 'tcp', hostIp: '127.0.0.1' },
      { private: 22, public: 2224, protocol: 'tcp', hostIp: '127.0.0.1' },
    ],
    compose: { project: 'gitlab-sandbox', service: 'gitlab', workingDir: `${HOME}/scripts/gitlab-sandbox`, configFiles: [GITLAB] },
  }),
  row({
    name: 'gitlab-sandbox-runner-1',
    image: 'gitlab/gitlab-runner:v17.3.1',
    state: 'restarting',
    status: 'Restarting (1) 12 seconds ago',
    created: at(26 * 3600),
    compose: { project: 'gitlab-sandbox', service: 'runner', workingDir: `${HOME}/scripts/gitlab-sandbox`, configFiles: [GITLAB] },
  }),
  row({
    name: 'atlas-postgres',
    image: 'postgres:16-alpine',
    status: 'Up 2 days',
    created: at(2 * 86400),
    ports: [{ private: 5432, public: 5433, protocol: 'tcp', hostIp: '127.0.0.1' }],
  }),
  row({
    name: 'appimage-build-jammy',
    image: 'cide-build:ubuntu-22.04',
    state: 'exited',
    status: 'Exited (0) 3 hours ago',
    created: at(4 * 3600),
  }),
  row({
    name: 'ra-bench',
    image: 'rust:1.84-bookworm',
    state: 'exited',
    status: 'Exited (137) 5 days ago',
    created: at(5 * 86400),
  }),
]

const MB = 1024 * 1024
function image(tag: string, mb: number, ago: number, containers: number): ImageRow {
  return {
    id: `sha256:${hexId(tag)}`,
    tags: tag === '' ? [] : [tag],
    created: at(ago),
    size: wire(Math.round(mb * MB)),
    containers: wire(containers),
    kind: 'image',
  }
}

const IMAGES: ImageRow[] = [
  image('cide-relay:dev', 38.4, 45 * 60, 1),
  image('gitlab/gitlab-ce:17.3.1-ce.0', 3_412, 9 * 86400, 1),
  image('gitlab/gitlab-runner:v17.3.1', 812, 9 * 86400, 1),
  image('cide-build:ubuntu-22.04', 2_146, 6 * 3600, 1),
  image('rust:1.84-bookworm', 1_487, 12 * 86400, 1),
  image('postgres:16-alpine', 251, 20 * 86400, 1),
  image('caddy:2.8', 49.7, 30 * 86400, 1),
  image('redis:7.4-alpine', 41.2, 30 * 86400, 1),
  // A dangling layer from the last relay rebuild — Docker's `-1` for "the daemon did not count".
  { ...image('', 38.1, 50 * 60, 0), containers: wire(-1) },
]

export const DOCKER_BOARD: DockerBoard = {
  kind: 'ready',
  endpoint: 'unix:///var/run/docker.sock',
  context: 'default',
  contexts: [
    { name: 'default', description: 'Current DOCKER_HOST based configuration', endpoint: 'unix:///var/run/docker.sock', current: true },
    { name: 'rootless', description: 'Rootless mode', endpoint: 'unix:///run/user/1000/docker.sock', current: false },
    { name: 'buildbox', description: 'The shared build machine', endpoint: 'ssh://dev@buildbox', current: false },
  ],
  apiVersion: '1.47',
  server: 'Docker Engine - Community 27.3.1',
  containers: CONTAINERS,
  images: IMAGES,
  volumes: [
    { name: 'cide-remote_redis', driver: 'local', mountpoint: '/var/lib/docker/volumes/cide-remote_redis/_data', project: 'cide-remote', inUseBy: wire(1) },
    { name: 'cide-remote_caddy-data', driver: 'local', mountpoint: '/var/lib/docker/volumes/cide-remote_caddy-data/_data', project: 'cide-remote', inUseBy: wire(1) },
    { name: 'gitlab-sandbox_config', driver: 'local', mountpoint: '/var/lib/docker/volumes/gitlab-sandbox_config/_data', project: 'gitlab-sandbox', inUseBy: wire(1) },
    { name: 'gitlab-sandbox_data', driver: 'local', mountpoint: '/var/lib/docker/volumes/gitlab-sandbox_data/_data', project: 'gitlab-sandbox', inUseBy: wire(1) },
    { name: 'atlas-pgdata', driver: 'local', mountpoint: '/var/lib/docker/volumes/atlas-pgdata/_data', inUseBy: wire(1) },
    { name: 'cargo-registry', driver: 'local', mountpoint: '/var/lib/docker/volumes/cargo-registry/_data' },
  ],
  networks: [
    { id: hexId('net-remote'), name: 'cide-remote_default', driver: 'bridge', scope: 'local', project: 'cide-remote', subnets: ['172.21.0.0/16'] },
    { id: hexId('net-gitlab'), name: 'gitlab-sandbox_default', driver: 'bridge', scope: 'local', project: 'gitlab-sandbox', subnets: ['172.22.0.0/16'] },
    { id: hexId('net-bridge'), name: 'bridge', driver: 'bridge', scope: 'local', subnets: ['172.17.0.0/16'] },
    { id: hexId('net-host'), name: 'host', driver: 'host', scope: 'local', subnets: [] },
    { id: hexId('net-none'), name: 'none', driver: 'null', scope: 'local', subnets: [] },
  ],
  compose: { kind: 'present', version: 'Docker Compose version v2.29.7' },
}

/** What `docker_detail` answers for the relay, the row the scene selects. */
export const RELAY_DETAIL: DockerDetail = {
  kind: 'container',
  id: RELAY.id,
  name: RELAY.name,
  image: RELAY.image,
  state: RELAY.state,
  status: RELAY.status,
  created: RELAY.created,
  command: '/srv/relay/cide-relay --config relay.toml',
  ports: RELAY.ports,
  mounts: [
    { kind: 'bind', name: '', source: `${HOME}/packaging/remote/relay.toml`, destination: '/srv/relay/relay.toml', readOnly: true },
    { kind: 'volume', name: 'cide-remote_sessions', source: '/var/lib/docker/volumes/cide-remote_sessions/_data', destination: '/srv/relay/sessions', readOnly: false },
  ],
  env: [
    { name: 'RUST_LOG', value: 'info,cide_pty=debug' },
    { name: 'CIDE_RELAY_RING_BYTES', value: '4194304' },
    { name: 'CIDE_RELAY_BACKPRESSURE', value: '8' },
    { name: 'PATH', value: '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin' },
  ],
  labels: [
    { name: 'com.docker.compose.project', value: 'cide-remote' },
    { name: 'com.docker.compose.service', value: 'relay' },
    { name: 'org.opencontainers.image.source', value: 'packaging/remote/Dockerfile' },
  ],
  networks: ['cide-remote_default'],
  restartPolicy: 'unless-stopped',
  ...(RELAY.compose ? { compose: RELAY.compose } : {}),
}

// --- what the two container panes show ------------------------------------------------

const CSI = '\x1b['
const paint = (code: number) => (s: string) => `${CSI}${code}m${s}${CSI}0m`
const green = paint(32)
const yellow = paint(33)
const blue = paint(34)
const cyan = paint(36)
const dim = paint(2)
const bold = paint(1)

/** An exec shell inside the relay, after a look around. */
export function execScreen(rows: number): string {
  const prompt = `${green('root@3f9c0e1a7b42')}:${blue('/srv/relay')}# `
  const lines = [
    `${prompt}ls -la`,
    'total 24',
    'drwxr-xr-x 1 relay relay 4096 Sep 24 14:02 .',
    'drwxr-xr-x 1 root  root  4096 Sep 24 14:02 ..',
    '-rw-r--r-- 1 relay relay  612 Sep 24 14:02 relay.toml',
    `-rwxr-xr-x 1 relay relay 9.6M Sep 24 14:02 ${green('cide-relay')}`,
    `drwx------ 2 relay relay 4096 Sep 24 14:17 ${blue('sessions')}`,
    `${prompt}cat relay.toml`,
    '[listen]',
    'addr = "0.0.0.0:8443"',
    '',
    '[sessions]',
    'ring_bytes = 4_194_304   # per session, see cide-pty',
    'idle_timeout = "30m"',
    'backpressure_chunks = 8',
    '',
    '[redis]',
    'url = "redis://redis:6379/0"',
    `${prompt}ps -o pid,rss,etime,args`,
    'PID   RSS  ELAPSED COMMAND',
    '  1  21m    42:07 /srv/relay/cide-relay --config relay.toml',
    ' 38 3.1m    00:04 /bin/bash',
    ' 51 1.2m    00:00 ps -o pid,rss,etime,args',
    `${prompt}${CSI}7m ${CSI}27m`,
  ]
  return lines.slice(Math.max(0, lines.length - rows)).join('\r\n')
}

/** The relay's log stream: two sessions attaching, and backpressure doing its job. */
export function logsScreen(rows: number): string {
  const t = (s: string) => dim(`14:${s}`)
  const info = green('INFO')
  const warn = yellow('WARN')
  const lines = [
    `${t('17:02.114')} ${info} ${cyan('relay')}: listening on 0.0.0.0:8443`,
    `${t('17:02.118')} ${info} ${cyan('store')}: redis connected`,
    `${t('18:40.551')} ${info} ${cyan('session')}: attach 7c1e client=phone 92x48`,
    `${t('18:40.552')} ${info} ${cyan('session')}: replay 61 440 bytes, 112 frames`,
    `${t('19:13.007')} ${info} ${cyan('session')}: attach 0b44 client=laptop 188x52`,
    `${t('21:55.930')} ${warn} ${cyan('coalesce')}: 0b44 8 chunks behind, ${bold('pausing')}`,
    `${t('21:56.012')} ${info} ${cyan('coalesce')}: 0b44 drained, resumed after 82ms`,
    `${t('22:31.448')} ${warn} ${cyan('coalesce')}: 0b44 8 chunks behind, ${bold('pausing')}`,
    `${t('22:31.501')} ${info} ${cyan('coalesce')}: 0b44 drained, resumed after 53ms`,
    `${t('24:10.260')} ${info} ${cyan('session')}: detach 7c1e client=phone (backgrounded)`,
    `${t('25:47.803')} ${info} ${cyan('health')}: ok sessions=1 ring=2.2MiB rss=21.4MiB`,
    `${t('26:47.805')} ${info} ${cyan('health')}: ok sessions=1 ring=2.2MiB rss=21.4MiB`,
  ]
  return lines.slice(Math.max(0, lines.length - rows)).join('\r\n')
}
