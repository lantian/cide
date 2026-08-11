/**
 * Renders every story to static HTML and prints a digest of what came out.
 *
 * Driven by `ui/scripts/check-git-render.mjs`, which asserts on the digest. This is not
 * part of the app: nothing imports it, so it is tree-shaken out of the real bundle.
 *
 * Why it exists: `pnpm build` succeeding says only that the panel compiles, and the panel was
 * for a while a thing that compiled, mounted, and drew nothing at all. Rendering it under node
 * with `react-dom/server` is the cheapest honest answer to "does it actually paint", and it
 * covers the multi-repo, submodule and guard-bar states that a screenshot of a clean checkout
 * never would. The stories it renders are real `ChangesTree` payloads through the real
 * `normalizeStatus`, so a shape mismatch shows up here as zero rows.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { GitPanelView } from './GitPanel'
import { useGitPanel } from './useGitPanel'
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

/**
 * The view, with its model built beside it.
 *
 * `GitPanelView` takes the model as a prop so that the app's host can call `useGitPanel` once
 * and hand the same one to both the tree and its context menu. Here there is no menu, so the
 * two lines are a component — and *not* importing the host is the point: the host reaches the
 * window's keymap and theme through `@/store/workspace`, which touches `document` at import
 * time and would end this render before it started. See the header of `GitPanel.tsx`.
 */
function Story() {
  const git = useGitPanel(null)
  // `dark` because the fixture digests are about structure, not colour, and the icon file a
  // row points at is not something static markup is asserted on either way.
  return <GitPanelView project={null} git={git} iconTheme="dark" />
}

const digests = STORIES.map((story) => {
  // `storyFromQuery` reads `location.search` when the component first renders, so the
  // story is selected by rewriting it between renders rather than by a prop the app does
  // not have.
  ;(globalThis.location as unknown as { search: string }).search = `?git-story=${story}`
  const html = renderToStaticMarkup(<Story />)
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
