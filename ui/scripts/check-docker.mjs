/**
 * Checks `src/sidebar/DockerPanel/model.ts` — the pure core of the M41 panel — and pins its
 * vocabularies against the Rust that defines them.
 *
 * Same shape as `check-agents.mjs`, and for the same reason: this project has no JS test runner,
 * and the module is deliberately import-free so the TypeScript already in `node_modules` can
 * compile it standalone and node can import the result. If that compile ever needs a tsconfig,
 * something has added an import and the node-testability of the core has been lost.
 *
 * # The five failure classes this makes unrepresentable
 *
 * **A verb that drifts from Rust.** `ContainerAction` is an enum in
 * `crates/cide-ipc/src/docker.rs` and a frozen union in TypeScript. `model.ts` does not import
 * `generated.ts` — it restates the shape structurally so it can be compiled alone — so nothing
 * else in the build can see an eighth action arriving with no icon, no label and no state that
 * offers it. Adding one in Rust fails this check until the panel knows it.
 *
 * **An action offered in a state where it cannot mean anything.** `actionsFor` is the panel's
 * claim about what is worth showing, and the two ways to get it wrong are opposite: a `start` on
 * a running container is a button the daemon refuses with a sentence the user has to translate
 * into "that never applied", and an *empty* strip on a state cide has not heard of is a row with
 * no way to stop it. Both are asserted, in both directions, including for a state that does not
 * exist — because Docker adds states and the default arm is the one nobody exercises.
 *
 * **A count that cannot tell "nothing" from "nobody looked".** `runningCount` returns `null`
 * before a read has answered and a number afterwards, and the rail's badge draws nothing for
 * either — so the *only* way to see this inverted is to assert it here. `0` and `null` are
 * pinned as different values with the same appearance, which is the whole of the rail's rule.
 *
 * **A board that goes backwards.** `newerBoard` must drop a `unknown` landing on a real board,
 * and must return the **identical object** when it drops — a selector reading `s.board` compares
 * by reference, and a copy re-renders the panel for ever. Identity is asserted with `===`, not
 * with a deep compare, because a deep compare would pass on the bug.
 *
 * **A stack whose rows scatter.** `groupByCompose` is what turns eight containers into three
 * stacks and a stray, and its two orderings are load-bearing rather than cosmetic: the daemon
 * reports containers in a different order between reads, so an unsorted grouping shuffles the
 * panel under the user's cursor. Ungrouped rows must come **last** and carry no heading, because
 * M43 hangs `up`/`down` off a heading and a heading over containers that belong to no stack
 * would offer buttons that cannot work.
 *
 * # What this does NOT cover, and nothing here should be read as claiming
 *
 *   - that the panel renders. That is `check-docker-render.mjs`'s job, through Vite's SSR
 *     bundle, and a model can pass every assertion below while the component paints nothing.
 *   - that `adapt.ts` converts the wire's `bigint` fields correctly. Nothing here imports
 *     `generated.ts`; `tsc --noEmit` over the real `adapt.ts` is what pins the two together.
 *   - that `cide://docker-changed` reaches the store at all.
 *   - anything whatever about the daemon. `cargo test -p cide-docker` is that, and
 *     `./target/debug/cide-headless docker` is how a real one is looked at.
 *
 * Run: `pnpm --dir ui run check:docker`   (or `node ui/scripts/check-docker.mjs`)
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(tmpdir(), 'cide-docker-'))

let failed = 0
const fail = (what, detail) => {
  failed += 1
  console.error(`FAIL ${what}${detail === undefined ? '' : `\n  ${detail}`}`)
}
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) fail(what, `actual:   ${a}\n  expected: ${b}`)
}
const ok = (cond, what) => {
  if (cond !== true) fail(what)
}

const read = (rel) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), 'utf8')
const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

/** The body of a Rust item, found by its opening line and its `\n}` — `check-agents.mjs`'s. */
function rustBody(source, opening, what) {
  const start = source.indexOf(opening)
  if (start < 0) throw new Error(`could not find ${what}`)
  const end = source.indexOf('\n}', start)
  return source.slice(start, end)
}

/** The camelCase wire names of a Rust enum's variants — `check-agents.mjs`'s regex. */
function variants(source, opening, what) {
  const body = stripComments(rustBody(source, opening, what))
  const inner = body.slice(body.indexOf('{') + 1)
  return [...inner.matchAll(/^[ \t]+([A-Z][A-Za-z0-9]*)[ \t]*(\{|\(|,|$)/gm)].map(
    (m) => m[1].charAt(0).toLowerCase() + m[1].slice(1),
  )
}

const sorted = (list) => [...list].sort()

const container = (over = {}) => ({
  id: 'c'.repeat(64),
  name: 'thing',
  image: 'alpine',
  state: 'running',
  status: 'Up 2 hours',
  created: 0n,
  ports: [],
  ...over,
})

/*
 * The seam, compiled by rewriting its one outside import. (M55)
 *
 * `adapt.ts` is the only file this check compiles that is not import-free — it is *the* seam, and
 * what it imports from `@/ipc/generated` is types only. `tsc` refuses a `paths` mapping on the
 * command line (TS6064), and a generated tsconfig is a lot of machinery for one type alias, so
 * the import is rewritten to a local stub instead.
 *
 * The stub types the board as `any` deliberately: every assertion this enables is about a value
 * the type system says cannot exist, which is exactly the blind spot the wire keeps walking into.
 * That makes the file's own `.map((c) => …)` callbacks implicitly `any`, so this one compile
 * relaxes `noImplicitAny` — and only this one. The import-free modules are compiled below at full
 * strictness, so nothing they guarantee is weakened.
 */
const seam = join(out, 'seam')
mkdirSync(seam, { recursive: true })
writeFileSync(join(seam, 'generated-stub.ts'), 'export type DockerBoard = any\n')
writeFileSync(
  join(seam, 'adapt.ts'),
  readFileSync(join(UI, 'src/sidebar/DockerPanel/adapt.ts'), 'utf8').replaceAll(
    '@/ipc/generated',
    './generated-stub',
  ),
)
writeFileSync(
  join(seam, 'model.ts'),
  readFileSync(join(UI, 'src/sidebar/DockerPanel/model.ts'), 'utf8'),
)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      join(seam, 'adapt.ts'),
      join(seam, 'model.ts'),
      '--outDir', join(out, 'seamjs'),
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--noImplicitAny', 'false',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/sidebar/DockerPanel/model.ts',
      'src/panes/dockerFilesModel.ts',
      'src/sidebar/DockerPanel/detailModel.ts',
      'src/chrome/composeModel.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { cwd: UI, stdio: 'inherit' },
  )

  // `tsc` derives the common root from the inputs, so the two land under their own
  // directories — which is what keeps two files from colliding on one name.
  const model = await import(`file://${join(out, 'sidebar/DockerPanel/model.js')}`)
  const files = await import(`file://${join(out, 'panes/dockerFilesModel.js')}`)
  const detail = await import(`file://${join(out, 'sidebar/DockerPanel/detailModel.js')}`)
  const compose = await import(`file://${join(out, 'chrome/composeModel.js')}`)
  const {
    BOARD_UNKNOWN,
    actionsFor,
    groupByCompose,
    imageLabel,
    initiallyOpen,
    isOpen,
    isDestructive,
    isRunning,
    newerBoard,
    portSummary,
    runningCount,
    sizeText,
    stackBlocked,
    toneFor,
  } = model

  /* --- the verb vocabulary, pinned to Rust ---------------------------------------------- */

  const rust = read('../../crates/cide-ipc/src/docker.rs')
  const ACTIONS = variants(rust, 'pub enum ContainerAction {', 'ContainerAction')
  ok(ACTIONS.length >= 7, 'ContainerAction was scanned and is not empty')

  // Every verb Rust knows must be offered by at least one state, and every verb the panel
  // offers must be one Rust knows. A verb offered and unhandled is a button that rejects; a
  // verb handled and never offered is dead code that looks like a feature.
  const STATES = ['running', 'paused', 'restarting', 'created', 'exited', 'dead', 'removing']
  const offered = new Set()
  for (const state of STATES) for (const action of actionsFor(state)) offered.add(action)
  // `pause` and `unpause` both appear; the set below is every verb reachable from some state.
  eq(sorted([...offered]), sorted(ACTIONS), 'every ContainerAction is reachable, and no others')

  /* --- what each state offers ------------------------------------------------------------ */

  ok(!actionsFor('running').includes('start'), 'a running container is not offered Start')
  ok(actionsFor('running').includes('stop'), 'a running container is offered Stop')
  ok(!actionsFor('exited').includes('stop'), 'an exited container is not offered Stop')
  ok(actionsFor('exited').includes('start'), 'an exited container is offered Start')
  ok(
    !actionsFor('restarting').includes('start'),
    'a restarting container is already trying; Start would mean nothing',
  )
  ok(actionsFor('restarting').includes('kill'), 'and Kill is what Stop-did-not-take reaches for')
  eq([...actionsFor('removing')], [], 'a container being removed offers nothing — every verb races')

  // The arm nobody exercises. Docker adds states, and a strip that went empty for one would be a
  // row with no way to stop it.
  const unknown = actionsFor('a-state-docker-has-not-invented-yet')
  ok(unknown.length > 0, 'an unknown state still offers something')
  ok(unknown.includes('stop'), 'and Stop above all, which is what the row is reached for')

  // Only `remove` confirms, and it must — there is no undo.
  eq(
    sorted(ACTIONS.filter((a) => isDestructive(a))),
    ['remove'],
    'exactly one verb is destructive',
  )

  /* --- the badge: null and 0 are different --------------------------------------------- */

  eq(runningCount(BOARD_UNKNOWN), null, 'before a read has answered, nobody has looked')
  const ready = (containers) => ({ ...BOARD_UNKNOWN, kind: 'ready', containers })
  eq(runningCount(ready([])), 0, 'a live daemon with nothing running is zero, not null')
  eq(
    runningCount(ready([container(), container({ state: 'exited' })])),
    1,
    'and only the running ones are counted',
  )
  ok(
    isRunning(container({ state: 'restarting' })),
    'a container in a crash loop is emphatically not stopped — a badge that ignored it would ' +
      'read zero on a machine that is busy failing',
  )
  eq(runningCount({ ...BOARD_UNKNOWN, kind: 'absent' }), null, 'and a refusal is not a count')

  /* --- the drop rule, by reference ------------------------------------------------------- */

  const real = ready([container()])
  ok(
    newerBoard(real, BOARD_UNKNOWN) === real,
    'a real board is never replaced by "nobody has asked", and the IDENTICAL object comes back ' +
      '— a copy re-renders the panel for ever',
  )
  const next = ready([])
  ok(newerBoard(real, next) === next, 'a newer real board is adopted')
  ok(newerBoard(BOARD_UNKNOWN, next) === next, 'and the first read is always adopted')

  /* --- grouping -------------------------------------------------------------------------- */

  const composed = (project, service) =>
    container({ name: `${project}-${service}-1`, compose: { project, service } })
  const groups = groupByCompose([
    container({ name: 'stray' }),
    composed('shop', 'web'),
    composed('atlas', 'db'),
    composed('shop', 'api'),
  ])
  eq(
    groups.map((g) => g.label),
    ['atlas', 'shop', 'Other containers'],
    'stacks sort by name and the ungrouped rows come last — with a heading of their own since ' +
      'groups became collapsible, because a group with nothing to click cannot be reopened',
  )
  eq(
    groups.map((g) => g.stack),
    [true, true, false],
    'and only the real stacks claim to be one — `stack` is what withholds up/down from the ' +
      'ungrouped heading, now that "has a label" no longer answers it',
  )
  eq(
    groups[1].containers.map((c) => c.compose.service),
    ['api', 'web'],
    'and rows within a stack sort by service, so the panel does not shuffle between reads',
  )
  eq(groupByCompose([]).length, 0, 'no containers is no groups, not one empty one')

  // The bug this shipped with, as a test. `#[ts(optional)]` changes the emitted *type* and not
  // what serde writes, so an absent `compose` arrived as `null` while the type said `undefined`
  // — `=== undefined` was false, and the next property access threw `null is not an object`, on
  // the common case of a container with no compose labels. The DTO pairs `skip_serializing_if`
  // now and `adapt.ts` normalises on top; this asserts the model survives it either way, because
  // a crash here takes the whole panel down through its boundary.
  {
    const fromTheWire = [
      { ...container({ name: 'stray' }), compose: null },
      { ...container({ name: 'shop-web-1' }), compose: { project: 'shop', service: 'web', workingDir: null } },
    ]
    const groups = groupByCompose(fromTheWire)
    eq(
      groups.map((g) => g.label),
      ['shop', 'Other containers'],
      'a null `compose` is ungrouped rather than a TypeError',
    )
    eq(
      groups[0].workingDir,
      undefined,
      'and a null `workingDir` is undefined, never the string "null" or the null itself',
    )
    eq(runningCount({ ...BOARD_UNKNOWN, kind: 'ready', containers: fromTheWire }), 2)
  }

  /* --- the stack verbs, pinned to Rust ---------------------------------------------------- */

  const STACK = variants(rust, 'pub enum ComposeAction {', 'ComposeAction')
  eq(
    sorted(STACK),
    ['build', 'down', 'pull', 'recreate', 'restart', 'up'],
    'ComposeAction is the six compose verbs',
  )

  /*
   * The panel may only offer the **bounded** ones, and that is decided in Rust.
   *
   * `is_quick` is the table, scraped rather than restated: the panel's buttons wait for an answer
   * and report the board, so a verb that can legitimately spend ten minutes — a cold `pull` of a
   * multi-gigabyte image, a `build` — must not appear there. It belongs to the pane road, which
   * has a person watching it and a Ctrl+C that reaches the child.
   *
   * Both directions. A quick verb missing from the panel is a button somebody has to go looking
   * for in a menu; a long verb *present* is a button that appears to hang.
   */
  // The `matches!` arm itself, not the enclosing `impl`: `label()` sits beside it and names every
  // variant, so a looser scrape would answer "all six are quick" and assert nothing at all.
  const quickArm = /matches!\(\s*self,\s*([^)]*)\)/.exec(stripComments(rust))
  ok(quickArm !== null, 'is_quick is a `matches!` over the bounded verbs')
  const quick = [...quickArm[1].matchAll(/Self::([A-Z][A-Za-z0-9]*)/g)].map(
    (m) => m[1].charAt(0).toLowerCase() + m[1].slice(1),
  )
  ok(quick.length > 0, 'and it names them')

  // The array literal, bounded at its `]`. A fixed character window spilled into `STACK_ICON`
  // below it and read `play` as a verb.
  const panelSource = read('../src/sidebar/DockerPanel/DockerPanel.tsx')
  const panelArray = /const STACK_ACTIONS: readonly StackAction\[\] = \[([^\]]*)\]/.exec(panelSource)
  ok(panelArray !== null, 'the panel draws a literal list of stack verbs')
  const panelVerbs = [...panelArray[1].matchAll(/'([a-z]+)'/g)].map((m) => m[1])
  eq(
    sorted(panelVerbs),
    sorted(quick),
    'the panel offers exactly the verbs `is_quick` allows — no long one, and none missing',
  )
  for (const long of STACK.filter((verb) => !quick.includes(verb))) {
    ok(
      !panelVerbs.includes(long),
      `${long} is unbounded and must stay on the pane road, where its output is visible`,
    )
  }

  // `stackBlocked` is the gate on all three, and its contract is the one `canDispatch` states in
  // `check-agents.mjs`: exactly one of a green light and a non-empty sentence, never both,
  // never neither. A control that is drawn and inert is what this project has paid for
  // repeatedly.
  eq(stackBlocked(BOARD_UNKNOWN), 'no daemon', 'no board, no stack actions')
  eq(
    stackBlocked({ ...BOARD_UNKNOWN, kind: 'absent' }),
    'no daemon',
    'and a refusal is not a daemon either',
  )
  const withCompose = (present, detail) => ({
    ...BOARD_UNKNOWN,
    kind: 'ready',
    compose: { present, detail },
  })
  eq(stackBlocked(withCompose(true, 'v2.32.4')), null, 'a live daemon with the plugin is a green light')
  const missing = stackBlocked(withCompose(false, 'install docker-compose-plugin'))
  ok(
    typeof missing === 'string' && missing.length > 0,
    'and without the plugin it is a SENTENCE, not a silent false — the stack is read from the ' +
      'daemon and is real whether or not the CLI that acts on it exists, so a heading with ' +
      'silently missing buttons would read as a bug in cide',
  )

  /* --- a stack's working directory --------------------------------------------------------- */

  {
    // Taken from *any* member that carries the label, not the first: a stack whose first
    // container was recreated without labels would otherwise lose its directory and its buttons
    // while its siblings still knew.
    const bare = container({ name: 'shop-web-1', compose: { project: 'shop', service: 'web' } })
    const labelled = container({
      name: 'shop-db-1',
      compose: { project: 'shop', service: 'db', workingDir: '/home/u/shop' },
    })
    const [stack] = groupByCompose([bare, labelled])
    eq(stack.workingDir, '/home/u/shop', 'the directory survives one unlabelled member')
    const [none] = groupByCompose([bare])
    eq(none.workingDir, undefined, 'and is undefined when no member has it, never an empty string')
  }

  /* --- which groups start open ------------------------------------------------------------ */

  {
    // The rule: a stack with something running is open, everything else is shut. A machine with
    // eight stacks has one or two being worked in, and opening all of them buries those.
    const running = container({ name: 'live', state: 'running', compose: { project: 'live', service: 'a' } })
    const stopped = container({ name: 'old', state: 'exited', compose: { project: 'old', service: 'a' } })
    const groups = groupByCompose([running, stopped])
    const defaults = initiallyOpen(groups)
    eq([...defaults], ['stack:live'], 'only the stack with something running starts open')

    const live = groups.find((g) => g.label === 'live')
    const old = groups.find((g) => g.label === 'old')
    ok(isOpen(live, defaults, []), 'the live stack is open')
    ok(!isOpen(old, defaults, []), 'and the stopped one is not')

    // `toggled` is the *difference* from the rule, never an absolute set — which is what lets a
    // stack the user shut stay shut when a container in it starts, and one they opened stay open
    // when its last container stops.
    ok(!isOpen(live, defaults, ['stack:live']), 'a user can shut an open stack')
    ok(isOpen(old, defaults, ['stack:old']), 'and open a shut one')
    ok(
      isOpen(old, ['stack:old'], ['stack:old']) === false,
      'and the same toggle inverts whichever way the rule has since moved — a stack the user ' +
        'shut does not spring open the moment something in it starts',
    )
  }

  /* --- display helpers ------------------------------------------------------------------- */

  eq(
    portSummary([
      { private: 5432, public: 5433, protocol: 'tcp' },
      { private: 5432, public: 5433, protocol: 'tcp' },
      { private: 80, protocol: 'tcp' },
    ]),
    '5433→5432',
    'a dual-stack publish is one port, and an exposed-but-unpublished one is not shown at all',
  )
  eq(portSummary([]), '', 'and no ports is an empty string, not "none"')

  eq(sizeText(0n), '0 B', 'zero bytes')
  eq(sizeText(512n), '512 B', 'under a kilobyte stays in bytes')
  ok(sizeText(1610612736n).startsWith('1.5 GB'), 'and a real image size reads as one')

  eq(
    imageLabel({ id: 'sha256:a', tags: [], created: 0n, size: 0n, containers: 0n }),
    '<none>',
    'an untagged image is a dangling layer and says so rather than drawing an empty row',
  )

  /* --- tone ------------------------------------------------------------------------------ */

  eq(toneFor(container()), 'running', 'a running container')
  eq(
    toneFor(container({ health: 'unhealthy' })),
    'warning',
    'and an unhealthy one is a warning even while it runs — which is the point of a healthcheck',
  )
  eq(toneFor(container({ state: 'restarting' })), 'warning', 'a crash loop is a warning')
  eq(toneFor(container({ state: 'exited' })), 'stopped', 'an exited one is quiet')

  /* --- the file browser's path rules ------------------------------------------------------ */

  {
    const { activate, crumbs, join: joinPath, parent, sizeText: fileSize } = files

    // Always `/`, never a platform path API: the container is Linux whatever cide runs on.
    // `cide_docker::files::join` is the same rule in Rust and both are tested, because the two
    // are used at different ends of the same walk.
    eq(joinPath('/', 'etc'), '/etc', 'the root')
    eq(joinPath('', 'etc'), '/etc', 'and an empty base is the root')
    eq(joinPath('/etc', 'hosts'), '/etc/hosts')
    eq(joinPath('/etc/', 'hosts'), '/etc/hosts', 'a trailing slash does not double')

    eq(parent('/etc/nginx'), '/etc')
    eq(parent('/etc'), '/')
    eq(
      parent('/'),
      null,
      'and the root has no parent — `/` would make the first crumb click into itself, which is ' +
        'a control that appears to do nothing',
    )

    eq(
      crumbs('/etc/nginx').map((c) => c.path),
      ['/', '/etc', '/etc/nginx'],
      'the trail always starts at the root, which is the one directory every container has',
    )
    eq(crumbs('/').map((c) => c.label), ['/'], 'and the root alone is one crumb')

    // The decision worth pinning: a symlink is NOT followed. `GET /archive` does not follow one
    // either — it returns a link entry with no content — so descending into one would walk to a
    // path the reader then cannot open. `/bin/sh` on Alpine is a symlink, so this is the common
    // case rather than an edge one.
    const link = { name: 'sh', directory: false, link: '/bin/busybox' }
    eq(activate('/bin', link).kind, 'refuse', 'a symlink refuses rather than being followed')
    ok(
      activate('/bin', link).why.includes('/bin/busybox'),
      'and names what it points at, so the user can go there',
    )
    // Including a symlink *to a directory*, which `ls` reports as both.
    eq(
      activate('/', { name: 'lib', directory: true, link: 'usr/lib' }).kind,
      'refuse',
      'a symlink to a directory is still a symlink',
    )
    eq(activate('/', { name: 'etc', directory: true }).kind, 'descend')
    eq(activate('/etc', { name: 'hosts', directory: false }).kind, 'open')
    eq(
      activate('/etc', { name: 'hosts', directory: false }).path,
      '/etc/hosts',
      'and the path it carries is the joined one, not the bare name',
    )

    // `undefined` is not zero, and the two must not read alike: the daemon reported no size,
    // which happens for a directory and a device node, and `0 B` is a claim about an empty file.
    eq(fileSize(undefined), '', 'no size reported draws nothing')
    eq(fileSize(0), '0 B', 'and an empty file says so')
    eq(fileSize(102), '102 B')
    ok(fileSize(1536).startsWith('1.5 KB'), 'and a real size reads as one')
  }

  /* --- the file browser's context menu ----------------------------------------------------- */

  {
    const { ROW_ACTION_LABEL, entryInfo, rowActions } = files

    const file = { name: 'hosts', directory: false, size: 102 }
    const dir = { name: 'etc', directory: true }
    const link = { name: 'sh', directory: false, link: '/bin/busybox' }

    // Every action drawn must have a label, or the menu renders a row spelled `undefined` — and
    // the reverse, a label for an action nothing offers, is dead vocabulary that drifts.
    const offered = new Set([...rowActions(file), ...rowActions(dir), ...rowActions(link)])
    eq(
      sorted([...offered]),
      sorted(Object.keys(ROW_ACTION_LABEL)),
      'every row action has a label and every label is reachable',
    )
    for (const [action, label] of Object.entries(ROW_ACTION_LABEL)) {
      ok(label.length > 0, `${action} is labelled`)
    }

    // `Save to…` and not `Download`: the ellipsis is what says a dialog is coming, and the
    // download road asks (`docker_files_download` opens a save dialog in Rust before it reads a
    // byte). A label without it would promise a file appearing somewhere the user never chose.
    ok(ROW_ACTION_LABEL.download.endsWith('…'), 'the download is spelled as a question')

    // A **symlink offers no open and no download**, for `activate`'s reason one surface up:
    // `GET /archive` does not follow one, so an open refuses and a download hands back an empty
    // link entry. It keeps the two that still mean something.
    eq(sorted(rowActions(link)), sorted(['copyPath', 'info']), 'a symlink offers no open')
    ok(rowActions(dir).includes('download'), 'a directory downloads — as a tar, like `docker cp`')
    ok(rowActions(dir).includes('open'), 'and opens by descending into it')
    ok(rowActions(file).includes('open'), 'a file opens')

    // The order is the order drawn, and `open` first is what makes the menu agree with the
    // double-click every row already answers to.
    eq(rowActions(file)[0], 'open', 'open leads, matching the double-click')

    // Info draws a row per fact the listing actually carried, and never invents one. A blank
    // `Mode` would be a claim that the file has no permissions; `0 B` for an unreported size
    // would be a claim that it is empty. Both are the `undefined is not zero` rule again.
    const names = (entry, dirPath = '/etc') => entryInfo(dirPath, entry).map((l) => l.name)
    eq(
      names(file),
      ['Name', 'Path', 'Kind', 'Size'],
      'a bare listing line draws only what it knew',
    )
    eq(
      names({ ...file, mode: '-rw-r--r--', owner: 'root:root', modified: 'Sep 9 11:02' }),
      ['Name', 'Path', 'Kind', 'Size', 'Mode', 'Owner', 'Modified'],
      'and a full one draws all seven',
    )
    ok(!names(dir).includes('Size'), 'a directory reports no size, so no Size row')
    ok(names(link).includes('Target'), 'and a symlink names what it points at')

    const info = entryInfo('/etc', file)
    eq(
      info.find((l) => l.name === 'Path').value,
      '/etc/hosts',
      'the Path row is the joined path — the one thing Copy path also puts on the clipboard, ' +
        'and the two must not disagree',
    )
    eq(info.find((l) => l.name === 'Kind').value, 'file')
    eq(entryInfo('/', dir).find((l) => l.name === 'Kind').value, 'directory')
    eq(entryInfo('/bin', link).find((l) => l.name === 'Kind').value, 'symlink')
  }

  /* --- and the pane draws that menu -------------------------------------------------------- */

  {
    const pane = read('../src/panes/DockerFilesPane.tsx')

    // The three halves of a context menu, each silent on its own: a hook with no `onContextMenu`
    // never opens, an `onContextMenu` with no rendered `{menu}` opens nothing on screen, and
    // items built from anything but `rowActions` drift from the model this script just pinned.
    ok(pane.includes('useContextMenu('), 'the pane uses cide\'s own menu, not the browser\'s')
    ok(/onContextMenu=\{/.test(pane), 'and a row opens it')
    ok(pane.includes('event.preventDefault()'), 'suppressing the webview menu, which has no rows')
    ok(/\{menu\}/.test(pane), 'and the menu is rendered')
    ok(pane.includes('rowActions(entry)'), 'the items come from the model')
    ok(pane.includes('ROW_ACTION_LABEL['), 'and so do the labels')

    // The row the menu was opened on is read at open time, not stored in state: a re-render
    // between the right-click and the click is a render nobody needed, and the menu's own
    // `items` callback is the one place that needs the value.
    ok(pane.includes('menuEntry.current'), 'the clicked row rides a ref')

    // A cancelled save dialog answers `null`, which is an answer and not a failure. Reporting it
    // would put an error on screen for the user having changed their mind.
    ok(
      pane.includes('written !== null'),
      'a cancelled save dialog says nothing at all',
    )
  }

  /* --- the port syntax, both directions --------------------------------------------------- */

  {
    const { parsePorts, portsText } = detail

    // The round trip is the contract: what the field shows is what the field reads back, or a
    // user who changes nothing and presses Apply recreates their container with different ports.
    const cases = [
      [{ private: 80, public: 8080, protocol: 'tcp' }, '8080:80'],
      [{ private: 5432, public: 5433, protocol: 'tcp', hostIp: '127.0.0.1' }, '127.0.0.1:5433:5432'],
      [{ private: 53, public: 5353, protocol: 'udp' }, '5353:53/udp'],
      // Exposed but not published — `docker run --expose`. Distinguishable from `0:80`, which
      // asks the daemon for a random host port.
      [{ private: 80, protocol: 'tcp' }, '80'],
    ]
    for (const [port, text] of cases) {
      eq(portsText([port]), text, `${text} is how that port is written`)
      eq(parsePorts(text).problem, null, `${text} parses`)
      eq(parsePorts(text).ports, [port], `${text} round-trips`)
    }
    eq(portsText([]), '', 'no ports is an empty field, not a blank line')
    eq(parsePorts('').ports, [], 'and an empty field is no ports')
    eq(parsePorts('\n  \n').ports, [], 'blank lines are skipped rather than refused')

    // Refusals, each by name. A form that quietly mis-parsed would publish the wrong ports on a
    // container that has already been destroyed — there is no undo past that point.
    ok(parsePorts('8000-8010:8000-8010').problem?.includes('range'), 'a range is refused by name')
    ok(parsePorts('70000:80').problem?.includes('65535'), 'an out-of-range port is refused')
    ok(parsePorts('0:80').problem !== null, 'and zero is not a port')
    ok(parsePorts('8080:80/icmp').problem?.includes('icmp'), 'an unknown protocol is refused')
    ok(parsePorts('a:b:c:d').problem?.includes('colons'), 'and nonsense is refused')
    eq(
      parsePorts('8000-8010:8000').ports,
      [],
      'a refusal yields NO ports — a partial list applied to a recreate publishes some and ' +
        'drops the rest, on a container that is already gone',
    )
  }

  /* --- the panel is registered everywhere it has to be ------------------------------------ */

  // The panel lives in the **bottom tool window** since M46, not in the left sidebar. Five files,
  // and a miss in any one is silent in a different way: a tab with no row is a panel nobody can
  // reach, a row with no body is a tab that draws nothing, and a selection that reaches Rust is a
  // rejected command because `tool_window_activate` takes a uuid and `docker` is not one.
  const toolModel = read('../src/toolwindow/toolWindowModel.ts')
  ok(toolModel.includes("export const DOCKER_TAB = 'docker'"), 'the tab has a sentinel id')
  ok(toolModel.includes('isDockerTab'), 'and a guard that keeps it away from Rust')
  ok(
    /rows\.push\(\{\s*id: DOCKER_TAB/.test(toolModel),
    'and `tabRow` draws a row for it',
  )

  const host = read('../src/toolwindow/ToolWindowHost.tsx')
  ok(host.includes('<DockerPanel placement="bottom" />'), 'the host renders it, docked')
  ok(
    host.includes('if (isDockerTab(id)) {'),
    'and selects it in the webview — `tool_window_activate` takes a uuid, so `docker` must ' +
      'never be sent to Rust',
  )

  // And it is gone from the sidebar, in both directions. A `BuiltinView` nothing renders is a
  // rail button that does nothing; a rail button with no view is the same hole from the other
  // side. Asserted as *absence* because that is the half a move leaves behind.
  ok(
    !read('../src/chrome/sidebarView.ts').includes("| 'docker'"),
    'docker is no longer a BuiltinView',
  )
  ok(
    !read('../src/chrome/ActivityRail.tsx').includes("id: 'docker'"),
    'and the rail draws no button for it',
  )
  ok(
    !/\{sidebar\.view === 'docker'/.test(read('../src/App.tsx')),
    'and App.tsx renders no sidebar branch for it',
  )
  ok(
    !read('../../crates/cide-core/src/commands.rs').includes('"sidebar.docker"'),
    'and the sidebar command is gone from the registry — a command whose panel moved would be ' +
      'listed, bindable, and silently inert, which is what check:commands exists to prevent',
  )

  /* --- a stack carries the files it was brought up with ------------------------------------ */

  {
    /*
     * **The file list is not optional.** It was a hardcoded `[]` from M43 to M53, so every stack
     * action ran `docker compose -p <project> <verb>` with no `-f` and Compose fell back to
     * searching the working directory for a default-named file. That works by accident for
     * `compose.yaml` and `docker-compose.yml`, and a stack brought up from `docker.compose.yaml`
     * answered *"no configuration file provided: not found"* on every verb.
     *
     * The label exists so a tool does not have to guess, and it was already on the wire — the
     * adapter dropped it with a comment saying the stack actions "will" use it.
     */
    const withFiles = (over) =>
      container({ compose: { project: 'shop', service: 'web', configFiles: [], ...over } })
    const [stack] = groupByCompose([
      withFiles({ configFiles: [] }),
      withFiles({ service: 'db', configFiles: ['/srv/shop/compose.yaml', '/srv/shop/compose.dev.yaml'] }),
    ])
    eq(
      [...stack.files],
      ['/srv/shop/compose.yaml', '/srv/shop/compose.dev.yaml'],
      'the file list survives a member that was recreated without labels — `workingDir`\'s rule',
    )
    eq(
      [...groupByCompose([withFiles({})])[0].files],
      [],
      'and a stack whose members all lack the label is an empty list, never undefined',
    )

    // The panel must pass them. A four-argument callback that is called with three is not a type
    // error in JS, so `tsc` cannot see this one.
    const panel = stripComments(read('../src/sidebar/DockerPanel/DockerPanel.tsx'))
    ok(
      panel.includes('onStackAction(group.label, action, group.workingDir, group.files)'),
      'the stack buttons pass the file list, or Compose is left guessing at the file name',
    )
    const hostSource = stripComments(read('../src/sidebar/DockerPanel/DockerPanelHost.tsx'))
    ok(
      !/composeAct\([^)]*\[\]\s*\)/.test(hostSource),
      'and the host forwards them rather than passing an empty list — the M53 bug exactly',
    )
  }

  /* --- collapsing a group stays collapsed -------------------------------------------------- */

  {
    /*
     * The follow-a-link effect must fire once per **arrival of a selection**, not once per render.
     *
     * `groups` is recomputed on every render (`groupByCompose` is a plain call), so `defaults` is
     * a fresh array every time and a dependency list naming it fires constantly — re-opening the
     * selected row's heading the instant the user collapsed it. Reported as "I'm not able to
     * collapse the first group, it seems it auto expanding".
     *
     * Asserted on the guard rather than on behaviour, because nothing in this suite renders
     * twice: the ref is the mechanism, and its absence is the bug.
     */
    const panel = stripComments(read('../src/sidebar/DockerPanel/DockerPanel.tsx'))
    ok(
      /openedFor\s*=\s*useRef/.test(panel),
      'the effect remembers which selection it already opened for',
    )
    ok(
      /if \(openedFor\.current === key\) return/.test(panel),
      'and returns early for one it has already handled, so a board refresh cannot re-open a ' +
        'heading the user just collapsed',
    )
    ok(
      /\$\{selected\.kind\}:\$\{id\}/.test(panel),
      'compared by identity and not by reference — `selected` is rebuilt on every render of the ' +
        'host, so a reference test would be the same bug in a different spelling',
    )
  }

  /* --- an image's size is two numbers, and the panels must agree --------------------------- */

  {
    const { imageSizeRows } = detail
    const bytes = (n) => `${n}`

    /*
     * Reported as *"why is image size different in left and right panel?"* — `nginx:alpine` drawn
     * as 98 MB in the list and 28 MB in the detail. Both were a field called `Size`, and **the
     * daemon means something different by each** under the containerd image store. The real
     * numbers, from the machine it was reported on:
     *
     *   /images/json          102,437,698   ← the list, and what `docker images` prints
     *   /images/{id}/json      29,017,379   ← the detail
     *
     * The manifest settles it to the byte: the image is a 16-platform index with one platform
     * present, and 101,547,206 unpacked + 890,492 attestation = 102,437,698. The inspect number is
     * the compressed content.
     *
     * So the fix is not to pick a winner: it is that **the list's number is the one called Size**,
     * because that is what `docker images` says and what the row beside it shows, and the other is
     * labelled for what it is.
     */
    const rows = imageSizeRows(102437698, 29017379, bytes)
    eq(
      rows.map((r) => r.name),
      ['Size', 'Compressed'],
      'both numbers are drawn, and only one of them is called Size',
    )

    /*
     * **The second row explains itself without a hover**, which the first attempt did not.
     *
     * It was called `Download`, and the next question asked was *"and what is download size?"* —
     * the label saying what the number was *for* rather than what it *is*, and over-claiming
     * besides: under the containerd store those blobs stay in the content store after unpacking,
     * so they are not only a thing once transferred. A row nobody can read without asking is a
     * row that is named wrongly, and a tooltip is the second fix rather than the first.
     */
    ok(
      /as pulled|compressed/i.test(rows[1].value),
      `the value says what the number is, not only how big it is: ${rows[1].value}`,
    )
    for (const row of rows) {
      ok(
        typeof row.hint === 'string' && row.hint.length > 20,
        `${row.name} carries a sentence for the tooltip, because both are terms of art`,
      )
    }
    ok(/docker images/.test(rows[0].hint), 'and `Size` names the command it agrees with')
    eq(
      rows[0].value,
      '102437698',
      'and `Size` is the LIST\'s number, so the two panels agree and both match `docker images`',
    )
    ok(
      rows[1].value.startsWith('29017379'),
      `the compressed content is the second row's number: ${rows[1].value}`,
    )

    /*
     * On the **classic** image store the two are the same number, and drawing one value twice
     * under two labels would invent a distinction that does not exist there. The rule is about
     * the values, so cide never has to detect which storage driver is in use.
     */
    eq(
      imageSizeRows(102437698, 102437698, bytes).map((r) => r.name),
      ['Size'],
      'one row when the daemon means one thing',
    )

    // And an image the board no longer lists — removed between the click and the answer — draws
    // the one number there is rather than nothing.
    eq(imageSizeRows(undefined, 29017379, bytes).map((r) => r.name), ['Size'])
    eq(imageSizeRows(undefined, 29017379, bytes)[0].value, '29017379')

    // The board's number is what the row draws too, which is the other half of "the panels
    // agree": both sides read `ImageRow.size` and neither re-derives it.
    const host = stripComments(read('../src/sidebar/DockerPanel/DockerPanelHost.tsx'))
    ok(
      /imageOnDisk=\{/.test(host) && /board\.images/.test(host),
      'the detail is handed the board\'s size rather than asking the daemon a second question',
    )
  }

  /* --- a volume's size, and the three things it can say ------------------------------------ */

  {
    const { volumeSizeText } = detail
    const bytes = (n) => `${n} B`

    /*
     * **Three states, because "nobody measured" and "empty" are different claims.**
     *
     * `GET /volumes` carries no size at all — the only endpoint that does is `/system/df`, which
     * walks the filesystem: 8.4 seconds cold against a real daemon, about 1.3 warm. So the number
     * arrives late, and it sometimes does not arrive at all (a plugin driver, a remote daemon).
     * A volume drawn as `0 B` when nobody counted is a lie about somebody's data, which is the
     * same rule `ImageRow`'s `-1` and `VolumeRow`'s `inUseBy` already follow.
     */
    eq(volumeSizeText({ state: 'known', bytes: 0 }, bytes), '0 B', 'measured and empty says so')
    eq(volumeSizeText({ state: 'known', bytes: 1234 }, bytes), '1234 B')

    const unmeasured = volumeSizeText({ state: 'unmeasured' }, bytes)
    ok(!/\bB\b|\d/.test(unmeasured), `unmeasured must not read as a size: ${unmeasured}`)
    ok(/did not measure/i.test(unmeasured), 'and says who did not do what')

    // The waiting state names *why* it is slow. `docker system df` is the command a user would
    // reach for, and naming it makes a two-second wait read as a filesystem walk, not a hang.
    const measuring = volumeSizeText({ state: 'measuring' }, bytes)
    ok(/measuring/i.test(measuring), 'the waiting state says it is working')
    ok(/system df|filesystem/i.test(measuring), `and why it takes a moment: ${measuring}`)

    /*
     * And the cost is never paid by a board read. This is the assertion that matters most: the
     * event stream produces a refresh for every container that starts anywhere on the machine,
     * and a filesystem walk on each of those would be continuous.
     */
    const apiRs = stripComments(read('../../crates/cide-docker/src/api.rs'))
    const snapshotBody = apiRs.slice(apiRs.indexOf('pub(crate) async fn snapshot'), apiRs.indexOf('pub(crate) async fn volume_usage'))
    ok(
      !/volume_usage|\.df\(/.test(snapshotBody),
      'a board read never asks for disk usage — `/system/df` walks the filesystem, and the panel ' +
        'refreshes on every daemon event',
    )

    const host = stripComments(read('../src/sidebar/DockerPanel/DockerPanelHost.tsx'))
    ok(
      /volumeSize\(volumeName\)/.test(host),
      'the size is asked for by name, once a volume is selected',
    )
    ok(
      /cancelled = true/.test(host),
      'and a late answer for a volume nobody is looking at any more is dropped — the round trip ' +
        'is over a second and the selection can move twice inside it',
    )
  }

  /* --- removing an image, a volume or a network -------------------------------------------- */

  {
    const { removableFrom, removalPrompt } = model

    /*
     * **A container is not removable through this road**, and that is a type-level fact rather
     * than a runtime refusal. Removing one already has a road — `docker_container_action`'s
     * `Remove` — and it makes the *opposite* trade on purpose: it forces, because the frontend
     * has confirmed and a user who said "remove this running container" meant it. Everything
     * reachable through `Removable` refuses instead. Two roads to one gesture that disagree about
     * whether they force is the split `openPushDialog` exists to prevent.
     */
    eq(removableFrom({ kind: 'image', id: 'sha256:abc' }), { kind: 'image', id: 'sha256:abc' })
    eq(removableFrom({ kind: 'volume', name: 'data' }), { kind: 'volume', name: 'data' })
    eq(removableFrom({ kind: 'network', id: 'n1' }), { kind: 'network', id: 'n1' })
    eq(
      removableFrom({ kind: 'container', id: 'c1' }),
      null,
      'a container is removed through the forcing road, never this one',
    )

    // The daemon's asymmetry, which is the one way this silently removes nothing: a volume is
    // addressed by **name** and the other two by id.
    ok('name' in removableFrom({ kind: 'volume', name: 'data' }), 'a volume is named')
    ok('id' in removableFrom({ kind: 'network', id: 'n1' }), 'and a network is identified')

    /*
     * The confirmation names the **refusal**, not just the irreversibility.
     *
     * Docker declines while anything is using the thing, so that is the *likely* outcome rather
     * than the rare one — and a dialog that said only "this cannot be undone" would leave the
     * user reading an ordinary refusal as a bug.
     */
    const prompt = removalPrompt({ kind: 'image', id: 'sha256:abc' }, 'nginx:latest')
    ok(prompt.title.includes('nginx:latest'), 'the dialog names the thing that was clicked')
    ok(
      /refuse/i.test(prompt.body),
      'and says Docker will refuse if it is in use, which is the answer the user is most ' +
        'likely to get',
    )
    ok(/undone/i.test(prompt.body), 'while still saying the successful case is irreversible')

    // Rust's half: nothing forces, and the container arm does not exist to be forgotten.
    const apiRs = stripComments(read('../../crates/cide-docker/src/api.rs'))
    ok(
      /fn remove\(/.test(apiRs),
      'there is one removal function for the three kinds, so they cannot drift on force',
    )
    ok(
      /force\(false\)/.test(apiRs),
      'and it states `force(false)` rather than leaving it to a default a later reader could ' +
        'change without noticing what they changed',
    )
    ok(
      !/RemoveImageOptionsBuilder[\s\S]{0,120}force\(true\)/.test(apiRs),
      'nothing on this road forces — the daemon\'s refusal is the feature',
    )
  }

  /* --- a board that cannot be read says so, out loud --------------------------------------- */

  {
    /*
     * **The silent-failure shape that cost three rounds of "the panel does not update".**
     *
     * `adaptBoard`'s switch is exhaustive over `DockerBoard`, which is a statement about the
     * types and not about the bytes. Handed a payload that is not what the type says, it matched
     * nothing, returned `undefined`, and `newerBoard` threw one line later on a property of it.
     *
     * Down the command road that surfaces as a rejected `invoke`. Down the **event** road it is
     * silent — a throw inside a Tauri listener callback has nowhere to go but the webview
     * console — so the board stays as it was, manual Refresh goes on working because it is a
     * different call, and nothing anywhere says why.
     */
    /*
     * `tsc` emits `from './model'` and node's ESM loader wants the extension; the check compiles
     * to plain ESM rather than through a bundler, so the specifier is fixed up here. One line,
     * and it keeps this check free of a build step.
     */
    const seamOut = join(out, 'seamjs/adapt.js')
    writeFileSync(seamOut, readFileSync(seamOut, 'utf8').replace("from './model'", "from './model.js'"))
    const { adaptBoard } = await import(`file://${seamOut}`)

    for (const [bad, what] of [
      [undefined, 'nothing at all'],
      [{}, 'an object with no kind'],
      [{ kind: 'nope' }, 'an arm this build has not been taught'],
      // The specific mistake the shape invites: passing the event's whole payload rather than
      // its `board`. It has no `kind` of its own and used to come back as `undefined`.
      [{ board: { kind: 'ready' } }, "the event's wrapper instead of its board"],
    ]) {
      let threw = null
      try {
        const answer = adaptBoard(bad)
        ok(false, `adaptBoard(${what}) returned ${String(answer)} instead of refusing`)
      } catch (error) {
        threw = error
      }
      ok(
        threw instanceof Error && /Docker board/.test(threw.message),
        `adaptBoard refuses ${what} with a named error, rather than returning undefined for a ` +
          'caller to throw on somewhere else',
      )
    }

    // And the store turns that into something a user can see, rather than losing it in a
    // listener callback.
    const store = stripComments(read('../src/sidebar/dockerStore.ts'))
    ok(
      /catch \(error\) \{\s*notifyFailure\(error\)/.test(store),
      'and `adopt` reports it — a board that cannot be translated must not be indistinguishable ' +
        'from a daemon with nothing to report',
    )
  }

  /* --- the daemon is only read while somebody is looking ---------------------------------- */

  {
    /*
     * The subscription is an open socket and a **full board read** — containers, images, volumes,
     * networks — per daemon event. It was made lazy (`ensure_watching` starts it on the first
     * successful board read) and never stopped, so a session that opened the Docker tab once kept
     * reading the daemon for the rest of its life. Reported as continuous `bollard` DEBUG lines
     * in the log with the panel closed; it was really four API calls per event.
     *
     * Three things hold the fix together and each is silent on its own.
     */
    const host = read('../src/sidebar/DockerPanel/DockerPanelHost.tsx')
    const stateRs = read('../../crates/cide-app/src/docker_state.rs')
    const libRs = read('../../crates/cide-app/src/lib.rs')

    // The panel says it is on screen, **and says when it stops**. The second half is the one that
    // was missing for a year: starting lazily is worth nothing if it never ends.
    ok(host.includes('dockerApi.watch(true)'), 'the panel announces itself on mount')
    ok(host.includes('dockerApi.watch(false)'), 'and releases on unmount — the half that was missing')

    /*
     * **A level and a stamp, never an increment.** (M54)
     *
     * `docker_watch` is `async`, so Tauri runs these on its worker pool and they can be handled
     * out of order — and StrictMode makes that routine: mounting fires `true`, `false`, `true` in
     * one tick. A counter could not survive it in either direction. A decrement handled before
     * its increment is clamped at zero and **lost**, so the count drifts up and the subscription
     * is never stopped (measured: mount/unmount/mount ending at 2). And a stop whose action
     * overtook a start left a live panel with no subscription and nothing that re-reads the
     * count to recover. Both shipped; both were reported as "the panel does not update".
     */
    const client = stripComments(read('../src/ipc/client.ts'))
    ok(
      /seq:\s*\(watchSeq \+= 1\)/.test(client),
      'every watch call carries a stamp, so Rust can discard one that lost a race',
    )
    ok(
      !/fetch_add|fetch_sub|AtomicUsize/.test(stripComments(stateRs)),
      'and Rust counts nothing — an unordered channel cannot carry edges, which is what a ' +
        'counter made of them is',
    )
    ok(
      /\*known >= seq/.test(stripComments(stateRs)),
      'a message older than what this window has already said is discarded',
    )
    ok(
      /let _gate = self\.watch_gate\.lock\(\);/.test(stripComments(stateRs)),
      'and the bookkeeping and the action are one step, or a stop can still overtake a start',
    )

    /*
     * And the log level, which is the other half of what was reported — and the more serious one.
     *
     * `bollard` logs every decoded response body at DEBUG, and a container's JSON carries its
     * `Env` verbatim: database passwords, API tokens, whatever the compose file sets. cide's log
     * is the file users are asked to send back when something breaks. A third-party crate's own
     * formatting cannot be redacted, so the level is the control.
     */
    ok(
      /\.level_for\("bollard", log::LevelFilter::Info\)/.test(stripComments(libRs)),
      'bollard is capped at Info, so response bodies — which carry container environments — do ' +
        'not accumulate in the log users are asked to send back',
    )
  }

  /* --- following a link from the detail pane ----------------------------------------------- */

  {
    const { isSectionKey, keysToOpenFor, resolveRef, sectionsInitiallyOpen } = model
    const ready = {
      ...BOARD_UNKNOWN,
      kind: 'ready',
      containers: [
        container({ id: 'c'.repeat(64), name: 'shop-web-1', compose: { project: 'shop', service: 'web' } }),
        container({ id: 'd'.repeat(64), name: 'stray' }),
      ],
      images: [{ id: 'sha256:abc123def456', tags: ['nginx:latest', 'nginx:1.27'] }],
      volumes: [{ name: 'shop_pgdata' }],
      networks: [{ id: 'n1', name: 'shop_default' }],
    }

    // Every kind resolves, and by the spelling the *detail pane actually draws*: an image by the
    // tag `docker inspect` reported, a network by name, a container by name.
    eq(resolveRef(ready, { kind: 'image', text: 'nginx:latest' }), {
      kind: 'image',
      id: 'sha256:abc123def456',
    })
    eq(
      resolveRef(ready, { kind: 'image', text: 'nginx:1.27' }),
      { kind: 'image', id: 'sha256:abc123def456' },
      'any of its tags, because inspect reports whichever one started the container',
    )
    eq(resolveRef(ready, { kind: 'volume', text: 'shop_pgdata' }), {
      kind: 'volume',
      name: 'shop_pgdata',
    })
    eq(resolveRef(ready, { kind: 'network', text: 'shop_default' }), { kind: 'network', id: 'n1' })
    eq(resolveRef(ready, { kind: 'container', text: 'shop-web-1' }), {
      kind: 'container',
      id: 'c'.repeat(64),
    })

    /*
     * **`null` is an ordinary answer**, and the caller must draw plain text for it. Half of these
     * legitimately point at nothing: a bind mount names a host path, an image may have been
     * removed since, a container may be gone by the time the pane is read. A link that looks like
     * a link and selects nothing is the listed-and-inert control this project keeps finding.
     */
    eq(resolveRef(ready, { kind: 'volume', text: '/srv/data' }), null, 'a bind mount is not a row')
    eq(resolveRef(ready, { kind: 'image', text: 'redis:7' }), null, 'nor an image that is gone')
    eq(resolveRef(ready, { kind: 'container', text: '' }), null, 'nor an empty name')
    eq(
      resolveRef({ ...BOARD_UNKNOWN }, { kind: 'image', text: 'nginx:latest' }),
      null,
      'and a board that is not ready resolves nothing at all',
    )

    // Nothing matches on a substring — `ngin` must not find `nginx:latest`.
    eq(resolveRef(ready, { kind: 'image', text: 'ngin' }), null)

    /*
     * A followed selection has to be **on screen**, and that is two openings, not one.
     *
     * An image lives under a heading that starts shut; a container reached from a network's
     * attached list can be inside a collapsed stack. Opening the section and forgetting the group
     * is the half that only shows up on the second case, which is why one function answers both.
     */
    eq(
      [...keysToOpenFor(ready, { kind: 'image', id: 'sha256:abc123def456' })],
      ['images'],
      'an image needs its section opened',
    )
    eq(
      [...keysToOpenFor(ready, { kind: 'container', id: 'c'.repeat(64) })],
      ['stack:shop'],
      'and a container needs its compose group opened',
    )
    eq(
      [...keysToOpenFor(ready, { kind: 'container', id: 'd'.repeat(64) })],
      ['loose'],
      'including the ungrouped heading, which is a heading like any other',
    )

    /*
     * The two key families share one `toggled` set, which is only safe because they cannot
     * collide: a group's key is `stack:<project>` or `loose`, never a bare word. Asserted, because
     * the day it stops being true a compose project called "images" folds the Images section.
     */
    ok(isSectionKey('images') && isSectionKey('volumes') && isSectionKey('networks'))
    ok(!isSectionKey('stack:shop') && !isSectionKey('loose'), 'a group key is never a section key')
    for (const key of keysToOpenFor(ready, { kind: 'container', id: 'c'.repeat(64) })) {
      ok(
        !isSectionKey(key),
        `${key} is a group key and must not be mistaken for a section, or the wrong defaults ` +
          'decide whether it is open',
      )
    }

    // And the rule the effect depends on: a section starts shut, so a followed image always has
    // something to open. If sections ever start open this assertion is the one that says so.
    ok(
      !sectionsInitiallyOpen().includes('images'),
      'Images starts shut, which is exactly why following a link has to open it',
    )
  }

  /* --- the two columns scroll apart ------------------------------------------------------- */

  {
    /*
     * Three declarations, and dropping any one of them puts the panel back on **one** scrollbar.
     *
     * `ToolWindow.module.css`'s `.body` is a single `overflow: auto` box wrapping whatever tab is
     * active — correct for a Log tab, one long list. This panel is two columns that each own
     * their scrolling, and that only resolves if the panel has a *definite* height to divide:
     * `flex: 1` of an auto-height parent is the content's height, so `.body` overflowed instead
     * and one scrollbar moved the list and the detail together. Selecting a row at the foot of
     * the list scrolled the detail off the top, which is precisely when it is wanted.
     *
     * Asserted as CSS text because nothing in this suite lays anything out: the render checks SSR
     * to a string and there is no box model anywhere. That is a real limit — these three say the
     * declarations are present, not that the result is two scrollbars — and it is still the
     * difference between a regression that is caught and one that is only ever reported.
     */
    const panelCss = read('../src/sidebar/DockerPanel/DockerPanel.module.css')
    const detailCss = read('../src/sidebar/DockerPanel/DetailPane.module.css')

    /*
     * The declarations of one rule, **with its comments removed first**.
     *
     * Without the strip this checks prose. `.docked`'s comment explains `height: 100%` in so many
     * words, so the regex below matched the explanation and the assertion passed with the
     * declaration deleted — caught by trying it. Same shape as `check:diff-render`'s note about
     * `diffBlame.ts` saying a package name in prose that survives into the emitted bundle: a
     * grep over source that has not had its comments taken out is a grep over documentation.
     */
    const rule = (css, selector) => {
      const bare = css.replace(/\/\*[\s\S]*?\*\//g, '')
      const at = bare.indexOf(`\n${selector} {`)
      return at === -1 ? null : bare.slice(at, bare.indexOf('\n}', at))
    }

    const docked = rule(panelCss, '.docked')
    ok(docked !== null, '.docked exists — it is the bottom tool window placement')
    ok(
      /height:\s*100%/.test(docked ?? ''),
      '.docked states `height: 100%`. Without a definite height the panel grows to its content ' +
        'inside the tool window\'s own scroller, `.columns` `flex: 1` resolves to that content ' +
        'height, and both columns move on one scrollbar',
    )

    const list = rule(panelCss, '.body')
    ok(
      /overflow-y:\s*auto/.test(list ?? '') && /min-height:\s*0/.test(list ?? ''),
      'the list column scrolls itself, and carries the `min-height: 0` a flex child needs before ' +
        '`overflow` means anything',
    )

    const detail = rule(detailCss, '.body')
    ok(
      /overflow-y:\s*auto/.test(detail ?? '') && /min-height:\s*0/.test(detail ?? ''),
      'and so does the detail column, independently',
    )

    /*
     * And the selection must survive a hover.
     *
     * `.rowSelected:hover` and `.row:hover` are both (0,2,0), so the cascade falls through to
     * source order — and the hover rule is 160 lines further down the file. The selected row went
     * grey under the pointer, at the one moment the user most needs to see which row it is.
     * Three classes deep wins regardless of order; relying on the ordering would make a later
     * reshuffle of that file a silent regression, which is the argument `EditorSurface`'s change
     * bars already make about `highlightActiveLineGutter`.
     */
    const panelRules = panelCss.replace(/\/\*[\s\S]*?\*\//g, '')
    ok(
      /\.row\.rowSelected:hover/.test(panelRules),
      'the selected row keeps its background under the pointer, at (0,3,0) so it cannot lose to ' +
        '`.row:hover` on source order',
    )
    ok(
      /\.row\.rowSelected:focus-within/.test(panelRules),
      'and under keyboard focus, which is the same disappearance reached with Tab',
    )
  }

  /* --- the three sections below the containers -------------------------------------------- */

  {
    const { SECTION_KEYS, SECTION_LABEL, openFor, sectionCount, sectionsInitiallyOpen } = model

    // Every key has a label and every label is reachable, `ROW_ACTION_LABEL`'s rule: a heading
    // spelled `undefined` and a label nothing draws are the two ways a table like this rots.
    eq(
      sorted([...SECTION_KEYS]),
      sorted(Object.keys(SECTION_LABEL)),
      'every section has a label and every label is reachable',
    )

    // **All three start shut.** The rule `initiallyOpen` states for stacks, applied to the lists
    // that are longest and acted on least — and it is a claim worth pinning, because the failure
    // it prevents (a panel that opens with sixty images above the containers) is exactly the
    // state the collapsible work was asked for.
    eq([...sectionsInitiallyOpen()], [], 'no section starts open')
    for (const key of SECTION_KEYS) {
      ok(!openFor(key, sectionsInitiallyOpen(), []), `${key} is shut by default`)
      ok(openFor(key, sectionsInitiallyOpen(), [key]), `${key} opens when the user opens it`)
    }

    // `toggled` is the *difference* from the default and never an absolute set — the same
    // mechanism the stacks use, and the same one meaning. A section and a stack sharing it is
    // deliberate: two open-state stores would be two answers to one question.
    ok(openFor('shop', ['shop'], []), 'a default-open key stays open with nothing toggled')
    ok(!openFor('shop', ['shop'], ['shop']), 'and shuts when the user shuts it')

    // The count is what a shut heading shows, so it has to come from the board rather than from
    // the rows — a heading that said a different number than it holds would be worse than none.
    const board = {
      ...BOARD_UNKNOWN,
      kind: 'ready',
      images: [{}, {}, {}],
      volumes: [{}],
      networks: [{}, {}],
    }
    eq(sectionCount(board, 'images'), 3)
    eq(sectionCount(board, 'volumes'), 1)
    eq(sectionCount(board, 'networks'), 2)
  }

  /* --- compose: the file test, the verbs, and where a gutter marker goes ------------------- */

  {
    const { COMPOSE_LABEL, COMPOSE_VERBS, baseName, composeTargets, isComposeFile, isComposePath } =
      compose

    /*
     * The same answers as `cide_docker::compose::is_compose_file`, which is the *other* end of
     * one gesture: this one decides whether to offer the action and that one is what actually
     * runs. A disagreement is a menu entry that appears and then refuses, or one that never
     * appears at all — and neither end can see the other, so both are pinned to this list.
     */
    for (const yes of [
      'compose.yaml',
      'compose.yml',
      'docker-compose.yml',
      'docker-compose.yaml',
      'compose.override.yaml',
      'docker-compose.prod.yml',
      'Docker-Compose.YAML',
      'shop-compose.yaml',
      // Refused until M51: its *first* segment is `docker`, and the rule only looked there. A
      // compose file Compose reads happily, with no gutter marker and nothing saying why.
      'docker.compose.yaml',
      'app.compose.prod.yml',
    ]) {
      ok(isComposeFile(yes), `${yes} is a compose file`)
    }
    for (const no of [
      'compose.json',
      'composer.yaml',
      'values.yaml',
      'compose',
      'Dockerfile',
      '.env',
      'a.compose.yaml.bak',
      'decompose.yaml',
      // A PHP manifest, and the reason the test is on a whole segment and not a prefix.
      'composer.lock.yaml',
    ]) {
      ok(!isComposeFile(no), `${no} is not`)
    }

    // A **path**, because that is what the tree, the editor and the palette all hold — and both
    // separators, because this one runs against host paths unlike `dockerFilesModel.join`.
    eq(baseName('/srv/shop/compose.yaml'), 'compose.yaml')
    eq(baseName('C:\\srv\\shop\\compose.yaml'), 'compose.yaml')
    ok(isComposePath('/srv/shop/compose.yaml'))
    ok(!isComposePath('/srv/compose.yaml/main.rs'), 'a directory named like one is not one')

    // Every verb has a label, every label says `Compose`, and `Save to…`'s rule applies here
    // too: a bare `Up` in a file tree's menu beside *Rename* reads as a direction.
    eq(
      sorted([...COMPOSE_VERBS]),
      sorted(Object.keys(COMPOSE_LABEL)),
      'every verb has a label and every label is reachable',
    )
    for (const verb of COMPOSE_VERBS) {
      ok(COMPOSE_LABEL[verb].startsWith('Compose '), `${verb} says what it is`)
    }
    eq(COMPOSE_VERBS[0], 'up', 'up leads — it is what somebody with the file open wants')

    /*
     * The gutter's scan. Generous by design and never silent: an extra marker offers a run
     * Compose refuses by name in the pane, and a missing one gives the user nothing.
     */
    const file = [
      '# a comment',
      'name: shop',
      'services:',
      '  web:',
      '    image: nginx',
      '    ports:',
      '      - "80:80"',
      '  db:',
      '    image: postgres',
      '',
      'volumes:',
      '  data:',
    ].join('\n')
    const targets = composeTargets(file)

    eq(targets[0].line, 1, 'line 1 is always the whole file')
    eq(targets[0].service, null)
    eq(
      targets.slice(1).map((t) => t.service),
      ['web', 'db'],
      'one marker per service, and nothing from a service body',
    )
    eq(targets.slice(1).map((t) => t.line), [4, 8], 'and on the right lines')

    // The two ways this goes wrong, both silent. A *body* key at a deeper indent must not become
    // a service — `ports` and `image` are the ones that would — and a top-level key after the
    // block must end it, or `data` under `volumes:` is offered as a service.
    const names = targets.map((t) => t.service)
    for (const wrong of ['image', 'ports', 'data']) {
      ok(!names.includes(wrong), `${wrong} is not a service: ${JSON.stringify(names)}`)
    }

    // A file with no `services:` still offers the whole-file marker: `docker compose build` on a
    // file that only extends another one is a real thing, and an empty gutter would say the file
    // is not a compose file at all.
    eq(composeTargets('name: shop\n').length, 1)

    // Half-typed, which is the state a gutter on a buffer being edited is in most of the time.
    // A YAML parser answers this with an exception; the scan keeps the markers it already had.
    const typing = composeTargets('services:\n  web:\n    image: ngi')
    eq(typing.map((t) => t.service), [null, 'web'])

    // Tabs are not indentation in YAML, so a tabbed line is left alone rather than guessed at.
    ok(
      !composeTargets('services:\n\tweb:\n').some((t) => t.service === 'web'),
      'a tab-indented key is not counted, because guessing a width puts markers in wrong places',
    )
  }

  /* --- and the four surfaces that reach those verbs ---------------------------------------- */

  {
    // Four surfaces, one runner. The gesture is identical in all of them and the ordering inside
    // `runCompose` is delicate in one place (the plan before the split, so a machine with no
    // Compose gets the sentence *instead of* an empty pane), so a second copy would be a second
    // chance to get that one place wrong.
    const tree = read('../src/sidebar/FileTree.tsx')
    const dispatch = read('../src/keys/dispatch.ts')
    const gutterSource = read('../src/editor/composeGutter.ts')
    const runner = read('../src/chrome/composeRun.ts')

    ok(tree.includes('runCompose('), 'the file tree reaches the runner')
    ok(tree.includes('isComposePath('), 'and offers the submenu by the same name test')
    ok(dispatch.includes('runCompose('), 'the palette reaches the runner')
    ok(dispatch.includes("case 'docker.compose.up':"), 'through a real case and not a fallthrough')
    ok(gutterSource.includes('runCompose('), 'the editor gutter reaches the runner')
    ok(
      /composePlan\(/.test(runner),
      'and the runner is the only thing that asks Rust for a plan',
    )

    // The plan is resolved **before** the split. Reversed, a machine with no Compose gets a pane
    // titled `compose up` that prints nothing and explains nothing.
    ok(
      runner.indexOf('composePlan(') < runner.indexOf('splitPane('),
      'the plan is resolved before the pane is split',
    )

    // The gutter is the one surface that names a service, and the only one: `argv_for` puts
    // service names last, and every other caller means the whole stack.
    ok(
      gutterSource.includes('target.service === null ? [] : [target.service]'),
      'the gutter narrows to one service, and the file-level marker to none',
    )
    ok(
      !tree.includes('runCompose(row.path, verb, ['),
      'and the tree never does — it knows a file and nothing about what is inside it',
    )

    /*
     * **The session is spawned before the pane is split**, which is the M50 fix and the one
     * ordering in this file worth a test of its own.
     *
     * The first cut carried the argv on the intent and parked it in `layout/spawnPlans.ts` for
     * the gap between the split committing and `TerminalPane` mounting — the road `forkPrimary`
     * and `mirror` take. A plan is recorded after `pane_split` *resolves*, while the pane it is
     * for can render as soon as the `workspace_changed` broadcast lands, which Rust sends before
     * the command returns. A mirror that loses its plan adopts a held session and looks fine; a
     * compose pane that lost its plan fell through to a **login shell**, reported as "compose up
     * does nothing, only opens an empty terminal".
     */
    ok(
      runner.indexOf('sessionApi.spawn(') < runner.indexOf('splitPane('),
      'the session is spawned before the pane is split, so no plan has to survive the gap',
    )
    ok(
      runner.includes("kind: 'adopt'"),
      'and the pane adopts it through `SplitIntent::Adopt`, whose `Pane::session` is durable',
    )
    ok(
      // `stripComments`, because the paragraph above this assertion's subject *names*
      // `spawnPlans.ts` while explaining why it no longer uses it — and a grep over source that
      // has not had its comments removed is a grep over documentation. Third time this file has
      // learned that lesson; see the `rule()` helper's note for the first two.
      !/spawnPlans|rememberSpawnPlan/.test(stripComments(runner)),
      'this road parks nothing in the spawn-plan map — that map is what it stopped relying on',
    )

    // Rust's half: a Shell pane that names the session and stores no argv.
    const pane = read('../../crates/cide-app/src/cmd/pane.rs')
    ok(
      /SplitIntent::Adopt \{ title, \.\. \} => \(PaneKind::Shell/.test(pane),
      "an adopted pane is a Shell pane — every gesture on it behaves like a shell's",
    )
    ok(
      /SplitIntent::Adopt \{ session, \.\. \} => Some\(\*session\)/.test(pane),
      'and it names its session durably, which is what replaced the plan',
    )

  }

  /* --- model.ts imports nothing ---------------------------------------------------------- */

  ok(
    !/^\s*import\s/m.test(read('../src/sidebar/DockerPanel/model.ts')),
    'model.ts imports nothing, which is what lets this script compile it standalone',
  )
  ok(
    !/^\s*import\s/m.test(read('../src/panes/dockerFilesModel.ts')),
    'and so does the file browser\'s model',
  )
  ok(
    !/^\s*import\s/m.test(read('../src/chrome/composeModel.ts')),
    'and so does the compose model, which is why this script can hold it to the Rust answers',
  )

  if (failed > 0) {
    console.error(`\ncheck-docker: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-docker: ok (${ACTIONS.length} container actions and ${STACK.length} stack actions ` +
      `pinned to Rust, ${STATES.length} states offered coherent verbs, badge/drop/grouping/` +
      `compose rules held)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
