/**
 * Renders the problems panel to static HTML and prints a digest of what came out.
 *
 * Driven by `ui/scripts/check-problems.mjs`. Not part of the app: nothing imports it, so it is
 * tree-shaken out of the real bundle. Modelled on `GitPanel/smokeEntry.tsx`, and for the same
 * reason it was written there — a panel in this repo has already compiled, mounted and drawn
 * nothing, and `tsc` plus `vite build` cannot tell the difference.
 *
 * `model.ts` is checked directly under node, so what this adds is the half the pure core
 * cannot speak for: that the JSX actually paints, that a class referenced as `styles.x` exists
 * in the stylesheet (a missing one resolves to `undefined` and React silently drops the
 * attribute), and that the un-wired row really is a `div` rather than a dead-looking button.
 *
 * The `rogue` story is the regression pin that matters most: a severity outside the four used
 * to make every counter read zero, so the panel printed "No problems found" as its headline
 * *and* as the group's count, above two rows the user could see.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import type { ProjectId } from '@/ipc/client'
import { ProblemsPanel } from './ProblemsPanel'
import type { Diagnostic, DiagnosticsSnapshot } from './model'

/** What the check script asserts on. Strings and counts, so a failure prints something legible. */
export interface ProblemsDigest {
  story: string
  /** The header's right-hand figure: `—` until something looked. */
  meta: string | null
  /** The one claim the panel makes about the workspace. */
  claim: string | null
  /** Present only in `unavailable`, where the answer is surprising enough to need it. */
  explainer: boolean
  /** One entry per group: `<path> :: <its own count line>`. */
  groups: string[]
  /** One entry per row: `<data-severity>|<message text>`. */
  rows: string[]
  /**
   * One entry per row: how many classes its severity glyph carries.
   *
   * Always 2 — `.glyph` plus the severity's colour. A row whose glyph drops to 1 has lost the
   * red/yellow/blue distinction, which is the only thing separating an error from a hint at a
   * glance, and nothing else in this digest would notice.
   */
  glyphClasses: number[]
  /** One entry per row: the glyph character actually printed. */
  glyphs: string[]
  /** `button` when a host wired `onOpenLocation`, `div` when it did not, `null` with no rows. */
  rowTag: 'button' | 'div' | null
  /**
   * Elements the panel rendered with `class=""` or no `class` at all.
   *
   * A CSS module is `Record<string, string>`, so `styles.typo` type-checks and evaluates to
   * `undefined`. This is how the missing `.group` rule was caught.
   */
  unclassed: number
}

const PROJECT = 'smoke-project' as unknown as ProjectId

const REAL: readonly Diagnostic[] = [
  { path: 'src/b.rs', line: 4, column: 9, severity: 'warning', message: 'unused variable' },
  {
    path: 'src/a.rs',
    line: 12,
    column: 5,
    severity: 'error',
    message: 'mismatched types',
    code: 'E0308',
  },
  { path: 'src/a.rs', line: 3, column: 1, severity: 'hint', message: 'consider borrowing' },
  { path: 'src/a.rs', line: 12, column: 1, severity: 'error', message: 'cannot find value' },
]

const READY: DiagnosticsSnapshot = { kind: 'ready', source: 'rust-analyzer', items: REAL }

/*
 * A severity no version of this code knows. Cast, not written as `Severity`, because that is
 * exactly the situation: the wire type says `Severity` and the producer says otherwise.
 */
const ROGUE: DiagnosticsSnapshot = {
  kind: 'ready',
  source: 'future-ls',
  items: [
    { path: 'src/a.rs', line: 1, column: 1, severity: 'catastrophe', message: 'the roof is on fire' },
    // A prototype key, which is the one an `?? fallback` does not catch: the table lookup
    // returns `Object.prototype.constructor` — a function — and `??` passes it straight
    // through to JSX, where React rejects it as a child, and into `className`, where it
    // stringifies to `function Object() { [native code] }`.
    { path: 'src/a.rs', line: 2, column: 1, severity: 'constructor', message: 'and the walls' },
  ] as unknown as Diagnostic[],
}

const STORIES: Array<[string, () => string]> = [
  ['no-project', () => renderToStaticMarkup(<ProblemsPanel project={null} />)],
  ['no-source', () => renderToStaticMarkup(<ProblemsPanel project={PROJECT} />)],
  [
    'scanning',
    () =>
      renderToStaticMarkup(
        <ProblemsPanel project={PROJECT} snapshot={{ kind: 'scanning', source: 'tsserver' }} />,
      ),
  ],
  [
    'clean',
    () =>
      renderToStaticMarkup(
        <ProblemsPanel
          project={PROJECT}
          snapshot={{ kind: 'ready', source: 'rust-analyzer', items: [] }}
        />,
      ),
  ],
  ['ready', () => renderToStaticMarkup(<ProblemsPanel project={PROJECT} snapshot={READY} />)],
  [
    'ready-wired',
    () =>
      renderToStaticMarkup(
        <ProblemsPanel project={PROJECT} snapshot={READY} onOpenLocation={() => {}} />,
      ),
  ],
  ['rogue', () => renderToStaticMarkup(<ProblemsPanel project={PROJECT} snapshot={ROGUE} />)],
]

const digests = STORIES.map(([story, render]) => digest(story, render()))
console.log(JSON.stringify(digests))

function digest(story: string, html: string): ProblemsDigest {
  const rows = [
    ...html.matchAll(/data-audit="problemsRow" data-severity="([^"]*)"[^>]*>(.*?)<\/(?:div|button)>/g),
  ].map((m) => `${m[1]}|${message(m[2] ?? '')}`)
  return {
    story,
    // Keyed off the hashed module class rather than a position in the header, so a reordered
    // header cannot make this silently read the title instead of the figure.
    meta: text(/class="[^"]*headerMeta[^"]*"[^>]*>([^<]*)</.exec(html)?.[1] ?? '') || null,
    claim: text(/data-audit="problemsClaim"[^>]*>([^<]*)</.exec(html)?.[1] ?? '') || null,
    explainer: html.includes('data-audit="problemsExplainer"'),
    groups: [
      ...html.matchAll(/class="[^"]*groupPath[^"]*"[^>]*>([^<]*)<[\s\S]*?groupCount[^"]*"[^>]*>([^<]*)</g),
    ].map((m) => `${m[1]} :: ${m[2]}`),
    rows,
    glyphClasses: [...html.matchAll(/class="([^"]*glyph[^"]*)"/g)].map(
      (m) => (m[1] ?? '').trim().split(/\s+/).filter(Boolean).length,
    ),
    glyphs: [...html.matchAll(/class="[^"]*glyph[^"]*"[^>]*>([^<]*)</g)].map((m) => m[1] ?? ''),
    rowTag: rowTag(html),
    unclassed: unclassed(html),
  }
}

/**
 * Elements the panel hooks for the audit but forgot to style.
 *
 * A CSS module is typed `Record<string, string>`, so `styles.group` type-checks even when the
 * stylesheet defines no `.group` — it just evaluates to `undefined`, and React then omits the
 * attribute rather than writing `class=""`. Nothing in `tsc`, `vite build` or a model-level
 * check can see that; this is what caught it. Restricted to `data-audit` elements because
 * those are exactly the ones this panel styles — `<li>`, `<b>` and `<code>` inside the
 * explainer are styled by descendant selectors and carry no class by design.
 */
function unclassed(html: string): number {
  return [...html.matchAll(/<[a-z]+((?:\s+[a-z-]+(?:="[^"]*")?)*)\s*>/g)].filter((m) => {
    const attrs = m[1] ?? ''
    return attrs.includes('data-audit=') && !/\sclass="[^"]*[^"\s][^"]*"/.test(attrs)
  }).length
}

/** The innermost `<span class="_message…">` of a row, as text. */
function message(rowInner: string): string {
  return text(/class="_message[^"]*"[^>]*>(.*?)<\/span>/.exec(rowInner)?.[1] ?? '')
}

/**
 * Which element a row is rendered as — the distinction the panel's whole "no dead controls"
 * argument rests on. Found by walking back from the first row hook to whichever opening tag
 * is nearer, rather than by matching a class string the bundler hashes.
 */
function rowTag(html: string): 'button' | 'div' | null {
  const at = html.indexOf('data-audit="problemsRow"')
  if (at === -1) return null
  return html.lastIndexOf('<button', at) > html.lastIndexOf('<div', at) ? 'button' : 'div'
}

/** Markup to readable text: drop tags, collapse whitespace. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}
