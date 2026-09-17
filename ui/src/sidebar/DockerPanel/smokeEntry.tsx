/**
 * Renders every Docker panel story to static HTML and prints one JSON digest line. (M41)
 *
 * Driven by `ui/scripts/check-docker-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle. Modelled on `AgentsPanel/smokeEntry.tsx`, and
 * written for the reason that one was: a panel in this repository has already compiled, mounted
 * and drawn nothing, and neither `tsc` nor `vite build` could tell the difference.
 *
 * `model.ts` is checked directly under node by `check-docker.mjs`, so what this adds is the half
 * the pure core cannot speak for. Five things, each pinned to a claim that is wrong on screen
 * rather than in a value:
 *
 *  - that the **unknown** board draws no prose at all — because "nobody has looked" and "this
 *    machine has no Docker" are opposite claims, and drawing the second in the frame before the
 *    first read answers tells a user with a running daemon that they have none;
 *  - that the two **failure** screens print their sentence *and* a Retry, and that the one with
 *    an endpoint prints it — a user reading "connection refused" needs to see what refused;
 *  - that a **stack heading** is drawn for each compose project and **not** for the ungrouped
 *    rows, since M43 hangs `up`/`down` off a heading;
 *  - that every action button carries an accessible **name** — the strip is icon-only and
 *    hidden until hover, so a button with no label is unreachable rather than merely unlabelled;
 *  - that a class referenced as `styles.x` exists in the stylesheet. A CSS module is typed
 *    `Record<string, string>`, so `styles.typo` type-checks, evaluates to `undefined`, and React
 *    drops the attribute in silence — or, inside a template literal, writes the literal token
 *    `undefined` into the class list. Both are counted below.
 */
import { renderToStaticMarkup } from 'react-dom/server'

import styles from './DockerPanel.module.css'
import { DockerPanel } from './DockerPanel'
import { DOCKER_STORIES, type DockerStoryName } from './fixture'

export interface DockerDigest {
  readonly story: string
  /** How many container rows, and how many image rows. */
  readonly containers: number
  readonly images: number
  /** The compose headings, in the order drawn. */
  readonly stacks: readonly string[]
  /**
   * The three headings below them — `Images 2`, `Volumes 2` — as `name:count`.
   *
   * Separate from `stacks` and not folded into it, because the two answer different questions and
   * a combined list would have gone on passing when the sections became collapsible: the count is
   * the whole point of a *shut* section, and nothing would have been looking at it.
   */
  readonly sections: readonly string[]
  /** Every `aria-label` in the story, so an unnamed icon button is visible to an assertion. */
  readonly labels: readonly string[]
  /** Whether any prose was drawn at all — the `unknown` story's whole assertion. */
  readonly prose: boolean
  /** Whether a Retry button was drawn. */
  readonly retry: boolean
  /** Whether the endpoint was printed. */
  readonly endpoint: boolean
  /** Whether the context switcher was drawn. */
  readonly switcher: boolean
  /** How many "Open a terminal in …" buttons, and how many "Follow the logs of …". */
  readonly terminals: number
  readonly logs: number
  /** Volume and network rows. */
  readonly volumes: number
  readonly networks: number
  /** Stack-heading buttons, and the "no compose" note when they are absent. */
  readonly stackButtons: number
  readonly noCompose: boolean
  /** The literal token `undefined` appearing in a class list — a `styles.typo`. */
  readonly undefinedClasses: number
}

const count = (html: string, pattern: RegExp) => [...html.matchAll(pattern)].length

function digest(story: DockerStoryName): DockerDigest {
  const board = DOCKER_STORIES[story]
  // The host's own rule, mirrored: the callback is passed only when the plugin is there. A
  // fixture that passed it unconditionally would be testing a combination the app never
  // produces — see `fixture.ts`'s `readyWithoutCompose`.
  const stackable = board.kind === 'ready' && board.compose.present
  const html = renderToStaticMarkup(
    <DockerPanel
      board={DOCKER_STORIES[story]}
      onRefresh={() => {}}
      onAction={() => {}}
      onUseEndpoint={() => {}}
      onOpenStream={() => {}}
      onInspect={() => {}}
      onStackAction={stackable ? () => {} : undefined}
    />,
  )
  // The body's text with tags removed, so "did this draw any prose" is a question about what a
  // person sees rather than about the markup.
  const text = html
    .replace(/<[^>]*>/g, ' ')
    .replace(/\s+/g, ' ')
    .replace('Docker', '')
    .trim()

  return {
    story,
    containers: count(html, /data-audit="dockerContainer"/g),
    images: count(html, /data-audit="dockerImage"/g),
    // Every group heading, stack or not — the title differs, so the name is read off the
    // toggle button's own text instead.
    /*
     * Split at the section markers, so the two heading families are read apart.
     *
     * `.sectionLabel` *composes* `.groupLabel`, so both class names are on the element and no
     * class-based test can tell them apart — which is why the section carries a `data-audit` and
     * this splits on it. Everything before the first marker is a compose heading; each chunk
     * after one begins with a section's.
     */
    stacks: [
      ...(html.split('data-audit="dockerSection"')[0] ?? '').matchAll(
        /class="[^"]*groupName[^"]*">([^<]+)</g,
      ),
    ].map((m) => m[1] ?? ''),
    sections: html
      .split('data-audit="dockerSection"')
      .slice(1)
      .map((chunk) => {
        const name = /class="[^"]*groupName[^"]*">([^<]+)</.exec(chunk)?.[1] ?? ''
        const shown = /class="[^"]*groupCount[^"]*">([^<]+)</.exec(chunk)?.[1] ?? ''
        return `${name}:${shown}`
      }),
    labels: [...html.matchAll(/aria-label="([^"]*)"/g)].map((m) => m[1] ?? ''),
    prose: text.length > 0,
    retry: html.includes('>Retry<'),
    endpoint: html.includes(DOCKER_STORIES[story].endpoint) && DOCKER_STORIES[story].endpoint !== '',
    volumes: count(html, /data-audit="dockerVolume"/g),
    networks: count(html, /data-audit="dockerNetwork"/g),
    /*
     * Every stack button, whatever it is called.
     *
     * The verb alternation used to be spelled out — `(Start|Stop|Restart)` — and that made the
     * count silently *narrower* than the thing it measures: adding *Recreate* to `STACK_ACTIONS`
     * drew a fourth button that this regex did not see, so `check:docker-render`'s
     * "all three verbs" assertion went on passing over four. A label-shaped match with the verb
     * left open cannot go stale that way.
     */
    stackButtons: count(html, /aria-label="[A-Za-z ]+ the [^"]+ stack"/g),
    noCompose: html.includes('>actions unavailable<'),
    terminals: count(html, /aria-label="Open a terminal in /g),
    logs: count(html, /aria-label="Follow the logs of /g),
    switcher: html.includes('Follow the current context'),
    undefinedClasses: count(html, /class="[^"]*\bundefined\b[^"]*"/g),
  }
}

/**
 * Every class the stylesheet declares, so the check can assert that the ones the component reads
 * are a subset. Exported as data rather than asserted here, `AgentsPanel/smokeEntry.tsx`'s split:
 * this file renders, the check script decides.
 */
export const DECLARED_CLASSES = Object.keys(styles)

const stories = Object.keys(DOCKER_STORIES) as DockerStoryName[]
console.log(JSON.stringify(stories.map(digest)))
