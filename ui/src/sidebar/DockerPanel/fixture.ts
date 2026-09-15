/**
 * Fixed boards for the Docker panel's render check. (M41)
 *
 * Values only — no imports beyond the model's types — so `check-docker-render.mjs` drives the
 * *pure view* with data that never changes. `AgentsPanel/fixture.ts`'s rule: a story whose data
 * came from a store or a clock would make a digest that differs between runs, and a digest that
 * differs between runs is a digest nobody trusts.
 *
 * The five stories are the five screens, and each exists because it is a different **claim**:
 * `unknown` claims nothing, `absent` claims this machine has no Docker, `unusable` claims a
 * named daemon would not answer, `empty` claims a live daemon holds nothing, and `ready` is the
 * only one with rows in it.
 */
import { BOARD_UNKNOWN, type Board, type Container, type Image } from './model'

const container = (over: Partial<Container> & Pick<Container, 'id' | 'name'>): Container => ({
  image: 'alpine:latest',
  state: 'running',
  status: 'Up 2 hours',
  created: 1_700_000_000n,
  ports: [],
  ...over,
})

const image = (over: Partial<Image> & Pick<Image, 'id'>): Image => ({
  tags: ['alpine:latest'],
  created: 1_700_000_000n,
  size: 5_872_026n,
  containers: 1n,
  ...over,
})

const READY: Board = {
  ...BOARD_UNKNOWN,
  kind: 'ready',
  message: '',
  endpoint: 'unix:///home/u/.colima/default/docker.sock',
  context: 'colima',
  contexts: [
    {
      name: 'colima',
      description: 'colima',
      endpoint: 'unix:///home/u/.colima/default/docker.sock',
      current: true,
    },
    {
      name: 'colima-ozon',
      description: 'colima [profile=ozon]',
      endpoint: 'unix:///home/u/.colima/ozon/docker.sock',
      current: false,
    },
  ],
  apiVersion: '1.53',
  server: 'Docker Engine - Community',
  containers: [
    // A stray, to prove the ungrouped rows draw with no heading and sort after the stacks.
    container({ id: 'a'.repeat(64), name: 'minikube', state: 'exited', status: 'Exited (255) 2 days ago' }),
    container({
      id: 'b'.repeat(64),
      name: 'shop-web-1',
      image: 'nginx:alpine',
      ports: [{ private: 80, public: 8080, protocol: 'tcp' }],
      compose: { project: 'shop', service: 'web', configFiles: ['/srv/shop/compose.yaml'] },
    }),
    // Healthy, and published — the ordinary row.
    container({
      id: 'c'.repeat(64),
      name: 'shop-db-1',
      image: 'postgres:16-alpine',
      health: 'healthy',
      status: 'Up 2 hours (healthy)',
      ports: [{ private: 5432, public: 5433, protocol: 'tcp' }],
      // Deliberately label-less beyond the pair: `groupByCompose` takes the file list from
      // *any* member that has one, and a stack whose middle container was recreated without
      // labels must not lose its actions. This is that member.
      compose: { project: 'shop', service: 'db', configFiles: [] },
    }),
    // A crash loop, which must draw as a warning rather than as stopped.
    container({
      id: 'd'.repeat(64),
      name: 'shop-worker-1',
      state: 'restarting',
      status: 'Restarting (1) 4 seconds ago',
      compose: { project: 'shop', service: 'worker', configFiles: [] },
    }),
  ],
  images: [
    image({ id: 'sha256:1111' }),
    // Dangling: no tags at all, and Docker's `-1` for "the daemon did not count".
    image({ id: 'sha256:2222', tags: [], containers: -1n, size: 1_610_612_736n }),
  ],
  volumes: [
    { name: 'shop_pgdata', driver: 'local', mountpoint: '/var/lib/docker/volumes/shop_pgdata/_data', project: 'shop', inUseBy: 1 },
    // No project, and a count the daemon did not give — the two states that must not draw like
    // their neighbours.
    { name: 'stray-volume', driver: 'local', mountpoint: '/var/lib/docker/volumes/stray/_data' },
  ],
  networks: [
    { id: 'n1'.padEnd(64, '0'), name: 'shop_default', driver: 'bridge', scope: 'local', project: 'shop', subnets: ['172.20.0.0/16'] },
    // `host` has no subnets at all, which is a fact rather than a gap.
    { id: 'n2'.padEnd(64, '0'), name: 'host', driver: 'host', scope: 'local', subnets: [] },
  ],
  compose: { present: true, detail: 'Docker Compose version v2.32.4' },
}

export const DOCKER_STORIES = {
  /** Before anybody has looked. Must draw a header and nothing else. */
  unknown: BOARD_UNKNOWN,
  /** No endpoint anywhere on the ladder. */
  absent: {
    ...BOARD_UNKNOWN,
    kind: 'absent',
    message:
      'cide found no Docker daemon. If `docker ps` works in a terminal, run `docker context ls`.',
    endpoint: '',
    contexts: [],
    apiVersion: '',
    server: '',
    containers: [],
    images: [],
  } as Board,
  /** A named daemon that would not answer. */
  unusable: {
    ...BOARD_UNKNOWN,
    kind: 'unusable',
    message: 'cide could not reach the Docker daemon: No such file or directory',
    endpoint: 'unix:///var/run/docker.sock',
    contexts: [],
    apiVersion: '',
    server: '',
    containers: [],
    images: [],
  } as Board,
  /** A live daemon holding nothing — which is not the same as any of the three above. */
  empty: { ...READY, containers: [], images: [], volumes: [], networks: [] } as Board,
  ready: READY,
  /**
   * The same live daemon on a machine with no `docker compose`.
   *
   * A sixth story rather than the fifth rendered with a callback withheld, because that is not a
   * state the app can be in: the host decides whether to pass `onStackAction` from *this* field,
   * so a board saying Compose is present and a host withholding the callback would be testing a
   * combination nothing produces. The claim under test is that the stack is still drawn.
   */
  readyWithoutCompose: {
    ...READY,
    compose: {
      present: false,
      detail: '`docker` is not on this app\'s PATH, so cide cannot run Compose commands.',
    },
  } as Board,
} as const

export type DockerStoryName = keyof typeof DOCKER_STORIES
