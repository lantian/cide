/**
 * Renders every story to static HTML and prints a digest of what came out.
 *
 * Driven by `ui/scripts/check-git-render.mjs`, which asserts on the digest. This is not
 * part of the app: nothing imports it, so it is tree-shaken out of the real bundle.
 *
 * Why it exists: this panel is not mounted by `App.tsx` yet — the sidebar that hosts it is
 * another agent's surface in this milestone — so `pnpm build` succeeding says nothing about
 * whether the panel renders. Rendering it under node with `react-dom/server` is the
 * cheapest honest answer to "does it actually paint", and it covers the multi-repo and
 * guard-bar states that a screenshot of a clean checkout never would.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { GitPanel } from './GitPanel'
import type { GitStoryName } from './fixture'

/** What the check script asserts on. Strings, so a failure prints something readable. */
export interface StoryDigest {
  story: GitStoryName
  /** One entry per tree row: `L<aria-level>:<aria-checked>`. */
  rows: string[]
  repoRows: number
  guard: string | null
  summary: string | null
  commitEnabled: boolean
}

const STORIES: GitStoryName[] = ['mock', 'guard', 'multi', 'empty']

const digests = STORIES.map((story) => {
  // `storyFromQuery` reads `location.search` when the component first renders, so the
  // story is selected by rewriting it between renders rather than by a prop the app does
  // not have.
  ;(globalThis.location as unknown as { search: string }).search = `?git-story=${story}`
  const html = renderToStaticMarkup(<GitPanel project={null} />)
  // `[^>]*>` skips the rest of the bar's own attributes; the bar holds only spans and
  // buttons, so the first `</div>` after it is its own.
  const guard = /data-audit="gitGuard"[^>]*>(.*?)<\/div>/.exec(html)?.[1]
  return {
    story,
    rows: [...html.matchAll(/data-audit="gitRow"[^>]*aria-level="(\d+)" aria-checked="(\w+)"/g)].map(
      (m) => `L${m[1]}:${m[2]}`,
    ),
    repoRows: [...html.matchAll(/data-audit="gitRow" data-kind="repo"/g)].length,
    guard: guard === undefined ? null : text(guard),
    summary: /data-audit="gitSummary"[^>]*>([^<]*)</.exec(html)?.[1] ?? null,
    commitEnabled: !/data-audit="gitCommit"[^>]*disabled/.test(html),
  } satisfies StoryDigest
})

console.log(JSON.stringify(digests))

/** Markup to readable text: drop tags, collapse whitespace. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}
