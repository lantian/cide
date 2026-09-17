/**
 * Renders the Docker panel's five stories through Vite's SSR bundle and asserts on what came out.
 *
 * `pnpm build` proves the panel compiles; `check-docker.mjs` proves `model.ts` decides the right
 * things. Neither can see a panel that compiles, mounts and **draws nothing** — which is a state
 * this repository has already shipped, and the reason `AgentsPanel/smokeEntry.tsx` exists. This
 * is that gate for M41.
 *
 * # The five failure classes this makes unrepresentable
 *
 * **A claim made before it is known.** The `unknown` board must draw no prose whatever. "Nobody
 * has looked" and "this machine has no Docker" are opposite claims, and the second one in the
 * frame before the first read answers tells a user with a running daemon that they have none.
 * Asserted on the rendered *text*, tags stripped, because a sentence hidden behind a class is
 * still a sentence on screen.
 *
 * **A failure with no way out.** Both failure screens must print their sentence and a Retry, and
 * the one that has an endpoint must print it — a user reading "connection refused" needs to see
 * what refused. A panel that shows a reason and no Retry is a dead end for a daemon that is
 * simply not started yet.
 *
 * **A heading over rows it cannot act on.** A compose stack gets a heading; the ungrouped rows
 * must not. M43 hangs `up`/`down`/`restart` off that heading, so a heading over containers that
 * belong to no stack would offer buttons that cannot work — and the ungrouped rows would then be
 * the ones nobody notices are wrong.
 *
 * **An icon-only button with no name.** The action strip is icons, and it is `visibility:
 * hidden` until the row is hovered or focused. A button with no accessible name there is not
 * merely unlabelled — it is unreachable, because there is no text to find it by and no tooltip
 * until the pointer is already on it.
 *
 * **A `styles.typo`.** A CSS module is typed `Record<string, string>`, so `styles.nothere`
 * type-checks, evaluates to `undefined`, and React drops the attribute in silence — or, inside a
 * template literal, writes the literal token `undefined` into the class list. Both are counted.
 *
 * # What this does NOT cover
 *
 *   - colour. No renderer here computes one; `check:theme` resolves the tokens and a human eye
 *     is the rest.
 *   - hover. The action strip's reveal is a CSS rule, and SSR has no pointer — so this asserts
 *     the buttons are *in the markup*, which is the half a bundle can see.
 *   - the daemon. `cargo test -p cide-docker` and `cide-headless docker` are that.
 *
 * Run: `pnpm --dir ui run check:docker-render`
 */
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const UI = resolve(import.meta.dirname, '..')

/*
 * Inside `node_modules/.cache`, not the system temp directory — `check-agents-render.mjs`'s
 * placement, and it is load-bearing rather than tidy. The SSR bundle keeps `react-dom` external,
 * so importing it resolves that package **relative to the bundle's own path**; from a temp
 * directory there is no `node_modules` above it and the import fails with `ERR_MODULE_NOT_FOUND`
 * before a single assertion runs.
 */
mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-docker-render-'))

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

/**
 * Bundle the smoke entry for SSR, run it, and return its digests keyed by story.
 *
 * `console.log` is captured rather than parsed off stdout so a stray log from a dependency
 * cannot become the digest; the *last* line is taken, which is the contract the entry follows.
 */
async function render() {
  execFileSync(
    'node',
    [
      'node_modules/vite/bin/vite.js',
      'build',
      '--ssr', 'src/sidebar/DockerPanel/smokeEntry.tsx',
      '--outDir', out,
      '--logLevel', 'error',
    ],
    { cwd: UI, stdio: 'inherit' },
  )
  const printed = []
  const log = console.log
  console.log = (line) => printed.push(line)
  let mod
  try {
    mod = await import(`file://${resolve(out, 'smokeEntry.js')}`)
  } finally {
    console.log = log
  }
  return {
    stories: Object.fromEntries(JSON.parse(printed.at(-1)).map((d) => [d.story, d])),
    declared: mod.DECLARED_CLASSES,
  }
}

try {
  const { stories, declared } = await render()
  eq(
    Object.keys(stories).sort(),
    ['absent', 'empty', 'ready', 'readyWithoutCompose', 'unusable', 'unknown'].sort(),
    'every story rendered',
  )

  /* --- nobody has looked ----------------------------------------------------------------- */

  const unknown = stories.unknown
  ok(
    unknown.prose === false,
    'the unknown board draws NO prose — "nobody has looked" and "this machine has no Docker" ' +
      'are opposite claims, and making the second early tells a user with a running daemon ' +
      'that they have none',
  )
  eq(unknown.containers, 0, 'and no rows')
  eq(unknown.retry, false, 'and no Retry, because there is nothing to retry yet')
  eq(unknown.switcher, false, 'and no switcher, because there is no daemon to switch from')

  /* --- the two failures ------------------------------------------------------------------ */

  for (const name of ['absent', 'unusable']) {
    const story = stories[name]
    ok(story.prose, `the ${name} board prints its sentence`)
    ok(story.retry, `and a Retry — a reason with no way out is a dead end for a daemon that is ` +
      `simply not started yet (${name})`)
    eq(story.containers, 0, `and no rows (${name})`)
  }
  ok(
    stories.unusable.endpoint,
    'the unusable board prints the endpoint it tried — "connection refused" without one is a ' +
      'sentence the user cannot act on, because the whole question is WHICH daemon refused',
  )

  /* --- a live daemon --------------------------------------------------------------------- */

  const ready = stories.ready
  // Three of the four: the `shop` stack is open because something in it is running, and the
  // ungrouped group — holding only an exited `minikube` — starts collapsed. That rule is the
  // point of the collapse behaviour and this is where it is asserted on the markup.
  eq(
    ready.containers,
    3,
    'a group with something running starts open; one with nothing running starts collapsed, so ' +
      'a machine with eight old stacks does not bury the one being worked in',
  )
  // Image rows are asserted with the other two sections below — all three start shut now.
  eq(
    ready.stacks,
    ['shop', 'Other containers'],
    'every group has a heading, the ungrouped one included — it had none until groups became ' +
      'collapsible, at which point a group with nothing to click was a group the user could not ' +
      'get back. `Group.stack` is what still withholds up/down from it',
  )
  ok(ready.switcher, 'and the context switcher is drawn when the store holds contexts')

  // M42's two buttons per row, and the asymmetry between them is the assertion.
  eq(
    ready.logs,
    ready.containers,
    'every container offers a log follow, running or not — a stopped one\'s logs are the whole ' +
      'reason somebody opens this panel after a crash, and `docker logs` on one works',
  )
  eq(
    ready.terminals,
    3,
    'but only the three that are running offer a terminal: `docker exec` on a stopped container ' +
      'is refused by the daemon, and a button that always fails is worse than one that is absent',
  )
  ok(!ready.retry, 'a working board offers no Retry')

  /* --- volumes, networks and the stack heading ------------------------------------------- */

  /*
   * **Images, Volumes and Networks draw a heading and no rows.** (M49)
   *
   * They were plain labels until M49 and not collapsible at all, which is what was reported; they
   * fold now and start **shut**, for `initiallyOpen`'s reason applied to the lists that are
   * longest and acted on least — this panel is about containers, and sixty images buries them.
   *
   * The count on the heading is the load-bearing half and is why `sections` carries it: a shut
   * section that did not say how much it was hiding would be indistinguishable from an empty one,
   * and the user would have no reason to open it. Asserted against the fixture's own totals, so a
   * heading that says a different number than it holds fails here.
   */
  eq(
    ready.sections,
    ['Images:2', 'Volumes:2', 'Networks:2'],
    'each section draws a heading carrying the number of rows it is hiding',
  )
  eq(ready.images, 0, 'and no image rows, because the section starts shut')
  eq(ready.volumes, 0, 'nor volume rows')
  eq(ready.networks, 0, 'nor network rows')
  /*
   * Derived from the panel's own list, not a literal.
   *
   * It was `3`, and the day *Recreate* was added the number should have moved and did not — the
   * counter's regex named the three verbs it knew, so a fourth button was drawn and not counted
   * and the assertion passed while describing the wrong panel. Reading the list is what makes
   * this measure the thing it claims to.
   */
  const STACK_VERBS = [
    ...(
      /const STACK_ACTIONS: readonly StackAction\[\] = \[([^\]]*)\]/.exec(
        readFileSync(
          fileURLToPath(new URL('../src/sidebar/DockerPanel/DockerPanel.tsx', import.meta.url)),
          'utf8',
        ),
      )?.[1] ?? ''
    ).matchAll(/'([a-z]+)'/g),
  ].map((m) => m[1])
  ok(STACK_VERBS.length > 0, 'the panel draws a literal list of stack verbs')
  eq(
    ready.stackButtons,
    STACK_VERBS.length,
    `the one *stack* heading carries every verb the panel offers (${STACK_VERBS.join(', ')}) — ` +
      'and the ungrouped heading beside it carries none, because `docker compose` cannot act on ' +
      'containers that belong to no project',
  )
  ok(!ready.noCompose, 'and says nothing about Compose being missing, because it is not')

  const noCompose = stories.readyWithoutCompose
  eq(
    noCompose.containers,
    ready.containers,
    'without the Compose plugin the stack is STILL DRAWN — it is read from the daemon labels ' +
      'and is a real thing whether or not the CLI that acts on it exists',
  )
  eq(noCompose.stackButtons, 0, 'but it offers no stack buttons')
  ok(
    noCompose.noCompose,
    'and says why, on screen — a heading whose controls are silently missing reads as a bug in ' +
      'cide rather than as a machine without a plugin',
  )
  eq(
    noCompose.terminals,
    ready.terminals,
    'and the per-container buttons are untouched: they are Engine API and need no plugin',
  )

  // Every action button is named. The strip is icon-only and hidden until hover, so an unnamed
  // button is unreachable rather than merely unlabelled.
  const actionLabels = ready.labels.filter(
    (l) =>
      l !== 'Refresh' &&
      l !== 'Docker context' &&
      !l.startsWith('Open a terminal in ') &&
      !l.startsWith('Follow the logs of '),
  )
  ok(actionLabels.length > 0, 'the action strip rendered at all')
  ok(
    actionLabels.every((l) => l.trim().length > 0),
    'and every button in it carries a non-empty accessible name',
  )
  ok(
    actionLabels.every((l) => /\s/.test(l)),
    'each naming the verb AND the container — "Stop" alone is ambiguous in a list of eight',
  )
  ok(
    actionLabels.some((l) => l.startsWith('Stop ')),
    'a running container offers Stop',
  )
  ok(
    actionLabels.some((l) => l.startsWith('Start ')),
    'and the exited one offers Start — actionsFor reached the markup, not just the model',
  )
  ok(ready.labels.includes('Refresh'), 'the header Refresh is named too')

  /* --- a live daemon holding nothing ----------------------------------------------------- */

  const empty = stories.empty
  eq(empty.containers, 0, 'the empty board draws no rows')
  eq(empty.volumes, 0, 'nor volumes')
  eq(empty.networks, 0, 'nor networks')
  ok(empty.prose, 'but says so — an empty panel and a broken one must not look alike')
  ok(
    empty.switcher,
    'and it keeps the switcher, because "wrong context" is the first thing to check when a ' +
      'daemon you know has containers reports none',
  )
  ok(!empty.retry, 'and offers no Retry, because nothing failed')

  /* --- styles.typo ----------------------------------------------------------------------- */

  ok(declared.length > 0, 'the stylesheet declared classes')
  for (const story of Object.values(stories)) {
    eq(
      story.undefinedClasses,
      0,
      `no class list contains the literal token "undefined" (${story.story}) — a styles.typo ` +
        `type-checks, and React writes it into the class or drops the attribute in silence`,
    )
  }

  if (failed > 0) {
    console.error(`\ncheck-docker-render: ${failed} failure(s)`)
    process.exit(1)
  }
  console.log(
    `check-docker-render: ok (${Object.keys(stories).length} stories, ${ready.containers} rows, ` +
      `${actionLabels.length} named actions, ${ready.volumes} volumes, ${ready.networks} ` +
      `networks, ${declared.length} classes declared)`,
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}
