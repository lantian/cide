/**
 * Renders every Agents panel story to static HTML and prints one JSON digest line. (M18)
 *
 * Driven by `ui/scripts/check-agents-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle. Modelled on `ProblemsPanel/smokeEntry.tsx`, and
 * written for the reason that one was: a panel in this repository has already compiled,
 * mounted and drawn nothing, and neither `tsc` nor `vite build` could tell the difference.
 *
 * `model.ts` is checked directly under node by `check-agents.mjs`, so what this adds is the
 * half the pure core cannot speak for. Six things, each pinned to a failure this repository
 * has actually had or to a rule a user stated out loud:
 *
 *  - that a role with nothing running renders no Open *element* anywhere in its row, and that a
 *    role whose only run is **queued** renders none either — for a different reason, and still
 *    not as a disabled button;
 *  - that **every** role row carries a Configure, including a role whose harness is missing and
 *    a role the roster does not define at all;
 *  - that the **empty** screen is one centred button into Settings and no longer a page of
 *    worked YAML;
 *  - that the **disabled** screen prints the config path in full, before the button;
 *  - that the **unknown** roster draws nothing — no prose, no button — because "nobody has
 *    looked" and "it is off" are opposite claims and the second one is under a button that
 *    writes a committed file;
 *  - that a class referenced as `styles.x` exists in the stylesheet. A CSS module is typed
 *    `Record<string, string>`, so `styles.typo` type-checks, evaluates to `undefined`, and
 *    React drops the attribute in silence — or, inside a template literal, writes the literal
 *    token `undefined` into the class list. Both are counted below.
 *
 * The role entries are **objects and not `|`-joined strings**, because a role row now carries
 * six facts that assertions want separately — its status word, its summary glyph, how many runs
 * it is drawing, which buttons are inside it, how many of those are Opens, and its refusal
 * sentence — and a positional string is where an assertion silently starts reading the wrong
 * field.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { AgentsPanelView } from './AgentsPanel'
import { AGENTS_STORIES, type AgentsStoryName } from './fixture'

/** What the check script asserts on. Strings and counts, so a failure prints something legible. */
export interface AgentsDigest {
  story: AgentsStoryName
  /** The header's right-hand figure. Empty string when `metaFigure` withheld it. */
  meta: string
  /** The panel's one claim, when it makes one. */
  claim: string | null
  /** Every rendered button's text, in document order. `roster-unknown` must have none. */
  buttons: string[]
  /** The whole panel as readable text — what the config-path and sentence greps run over. */
  text: string
  /** One entry per run row — activity lines and Recent alike: `<phase>|<glyph>|<row text>`. */
  rows: string[]
  /**
   * The time cell of every run row: what it draws, and what its `title` says.
   *
   * Its own field rather than a substring of [`rows`], because it is the one thing on a row
   * that is computed from a clock and the only end-to-end proof that `RunRow` calls
   * `workedFor` at all — the model's own tables cannot see a row that went on calling
   * `elapsed(nowMs, startedMs)`, which is precisely the bug being fixed.
   */
  times: { drawn: string; title: string }[]
  /** How many Open controls exist **as elements**. A resting role must contribute zero. */
  opens: number
  /** How many Configure buttons exist. Must equal the number of role rows, in every story. */
  configures: number
  /** Every button's accessible name — its `aria-label`, else its text. Icon-only controls
      (Configure, Integrate) have no text, so `buttons` alone cannot see them. (M89) */
  labels: string[]
  /** The text of every `agentsUsing` line — what each run is on. (M89) */
  using: string[]
  /** The tabs as `tab|pressed|text`, and which tab panels carry `hidden`. (M89) */
  tabs: string[]
  hiddenPanels: string[]
  /** Integrate's two halves, as the `data-run` of the row each sits in. (M89) */
  integrates: string[]
  integrateConfirms: string[]
  /** One entry per role row. See the header for why these are objects. */
  roles: AgentsRoleDigest[]
  /** Is the empty screen's centred block on screen, and what is inside it? */
  empty: boolean
  /** The texts of the buttons inside that centred block, in order. */
  emptyButtons: string[]
  /** How many stale-turn bars are on screen. */
  staleBars: number
  /** The texts of the stale bar's own controls, in order. */
  staleActions: string[]
  /** How many `<button>` elements the first stale bar contains. Must be exactly two. */
  staleBarButtons: number
  /** Every phase mark drawn, in order, by name — the fallback `circle-slash` included. */
  glyphs: string[]
  glyphSpins: string[]
  /**
   * Every mark in the subtree, by name, phase marks and control marks alike.
   *
   * Separate from {@link glyphs}, which is only the phase column. A row's pause/resume button is
   * a bare mark with no text, so "does this row offer Resume" cannot be asked of the button
   * list any more — it is asked here.
   */
  icons: string[]
  /**
   * Elements the panel hooks for the audit but forgot to style, plus every class attribute
   * carrying the literal token `undefined`.
   *
   * The second half is the one a bare "does it have a class" counter misses:
   * `` `${styles.row} ${styles.typo}` `` produces `class="_row_ab1 undefined"`, which passes
   * every check that only looks for an empty attribute.
   */
  unclassed: number
}

/** One role row, as the assertions want to read it. */
export interface AgentsRoleDigest {
  /** `AgentDef.id`, off `data-role`. */
  id: string
  /** The summary phase, or `'none'` for a role with nothing active. Off `data-phase`. */
  phase: string
  /** The summary mark's name. Never empty, for any input. */
  glyph: string
  /** What the row says the subagent is doing, in words. */
  status: string
  /** How many activity lines the row drew — one per active run. */
  runs: number
  /** How many Open controls are inside **this row**. */
  opens: number
  /** Every button text inside this row, in document order. */
  buttons: string[]
  /** Every button's accessible name inside this row. See the top-level `labels`. */
  labels: string[]
  /** How many Configure controls are inside **this row**. Exactly one. */
  configures: number
  /** The role name's `title` — its id, and the description when there is one. (M89) */
  nameTitle: string
  /** The refusal sentence, or `''` when `canDispatch` says yes. */
  reason: string
  /**
   * The source chip's text, or `''` when the row draws none. (M30)
   *
   * Sliced out of the row rather than counted across the document, because the assertion the
   * check makes is per row — *these* rows are badged and *that* one is not — and a document-wide
   * count could not tell a badge keyed on the scope from a chip drawn on everything.
   */
  badge: string
  /** The scope the chip claims, so the check can tell the two Claude scopes apart. */
  badgeScope: string
  /**
   * The inline colour on the role's name, verbatim — `var(--agent-…)`, or `''`. (M75)
   *
   * Read off the **style attribute** and not a class, because the colour is a value chosen per
   * role rather than one of a fixed set of rules; a class digest would be identical for every
   * row and would prove nothing. The `''` case is a failure the check names out loud: a name
   * with no inline colour is drawn in whatever it inherited, which is what every role looked
   * like before this existed.
   */
  nameColor: string
}

/**
 * The `color:` of the inline style on the element carrying this audit hook, or `''`.
 *
 * Two steps — find the tag, then read the attribute out of it — rather than one regex spanning
 * both, because JSX prop order decides attribute order in the markup and a single ordered
 * pattern silently matches nothing the day somebody moves `style` above `data-audit`. That is
 * exactly the trap `cell()`'s `data-current` rule in the diff pane is written about, arriving
 * here from the other side: there the fix was to freeze the order, and here there is no reason
 * to, so the reader is made order-blind instead.
 */
function inlineColor(html: string, audit: string): string {
  const tag = new RegExp(`<[a-z]+\\b[^>]*data-audit="${audit}"[^>]*>`).exec(html)?.[0] ?? ''
  return /style="[^"]*\bcolor:\s*([^";]*)/.exec(tag)?.[1]?.trim() ?? ''
}

const digests = (Object.keys(AGENTS_STORIES) as AgentsStoryName[]).map((story) =>
  digest(story, renderToStaticMarkup(<AgentsPanelView {...AGENTS_STORIES[story]} />)),
)
console.log(JSON.stringify(digests))

function digest(story: AgentsStoryName, html: string): AgentsDigest {
  const rows = all(html, 'agentsRow').map((row) => {
    const phase = attr(row, 'data-phase')
    const glyph = text(/data-audit="agentsGlyph"[^>]*>([^<]*)</.exec(row)?.[1] ?? '')
    return `${phase}|${glyph}|${text(row)}`
  })
  const roles: AgentsRoleDigest[] = all(html, 'agentsRole').map((role) => ({
    id: attr(role, 'data-role'),
    phase: attr(role, 'data-phase'),
    glyph:
      /data-audit="agentsRoleGlyph"[^>]*>\s*<svg[^>]*data-icon="([^"]*)"/.exec(role)?.[1] ?? '',
    status: text(/data-audit="agentsRoleStatus"[^>]*>([^<]*)</.exec(role)?.[1] ?? ''),
    runs: count(role, 'data-audit="agentsRow"'),
    opens: count(role, 'data-audit="agentsOpen"'),
    buttons: [...role.matchAll(/<button\b[^>]*>([\s\S]*?)<\/button>/g)].map((m) =>
      text(m[1] ?? ''),
    ),
    labels: labels(role),
    configures: count(role, 'data-audit="agentsConfigure"'),
    nameTitle: attr(/<[a-z]+\b[^>]*data-audit="agentsRoleName"[^>]*>/.exec(role)?.[0] ?? '', 'title'),
    reason: text(/data-audit="agentsRoleReason"[^>]*>([^<]*)</.exec(role)?.[1] ?? ''),
    badge: text(/data-audit="agentsRoleBadge"[^>]*>([^<]*)</.exec(role)?.[1] ?? ''),
    badgeScope:
      /data-audit="agentsRoleBadge"[^>]*\bdata-scope="([^"]*)"/.exec(role)?.[1] ?? '',
    nameColor: inlineColor(role, 'agentsRoleName'),
  }))
  const times = all(html, 'agentsElapsed').map((cell) => ({
    drawn: text(/>([^<]*)</.exec(cell)?.[1] ?? ''),
    title: attr(cell, 'title'),
  }))
  const empty = all(html, 'agentsEmpty')[0] ?? ''
  const bars = all(html, 'agentsStaleBar')
  return {
    story,
    meta: text(/data-audit="agentsMeta"[^>]*>([^<]*)</.exec(html)?.[1] ?? ''),
    claim: text(/data-audit="agentsClaim"[^>]*>([^<]*)</.exec(html)?.[1] ?? '') || null,
    buttons: [...html.matchAll(/<button\b[^>]*>([\s\S]*?)<\/button>/g)].map((m) => text(m[1] ?? '')),
    text: text(html),
    rows,
    times,
    opens: count(html, 'data-audit="agentsOpen"'),
    labels: labels(html),
    using: all(html, 'agentsUsing').map((line) => text(line)),
    tabs: all(html, 'agentsTab').map(
      (tab) => `${attr(tab, 'data-tab')}|${attr(tab, 'aria-pressed')}|${text(tab)}`,
    ),
    hiddenPanels: [...html.matchAll(/<div\b[^>]*data-tab="([^"]*)"[^>]*>/g)]
      .filter((m) => /\shidden(=""|\s|>|$)/.test(m[0]))
      .map((m) => m[1] ?? ''),
    integrates: rowsHolding(html, 'agentsIntegrate'),
    integrateConfirms: rowsHolding(html, 'agentsIntegrateConfirm'),
    configures: count(html, 'data-audit="agentsConfigure"'),
    roles,
    empty: empty !== '',
    emptyButtons: [...empty.matchAll(/<button\b[^>]*>([\s\S]*?)<\/button>/g)].map((m) =>
      text(m[1] ?? ''),
    ),
    staleBars: bars.length,
    staleActions: [...html.matchAll(/data-audit="agentsStaleAction"[^>]*>([\s\S]*?)<\/button>/g)].map(
      (m) => text(m[1] ?? ''),
    ),
    staleBarButtons: count(bars[0] ?? '', '<button'),
    icons: [...html.matchAll(/data-icon="([^"]*)"/g)].map((m) => m[1] ?? ''),
    glyphs: [
      ...html.matchAll(/data-audit="agentsGlyph"[^>]*>\s*<svg[^>]*data-icon="([^"]*)"/g),
    ].map((m) => m[1] ?? ''),
    /**
     * Each phase mark as `<icon>|spin` or `<icon>|still`.
     *
     * The class, not the name. `loader-circle` was the right mark everywhere and animated
     * nowhere, so `glyphs` above — which reads names — passed a static spinner for a milestone
     * and a half. See `model.ts::SPINNING_GLYPH`.
     */
    /* The role's summary dot is read too (M89). It drew the spinner and never turned, and
       nothing noticed, because this regex once matched `agentsGlyph` alone. */
    glyphSpins: [...html.matchAll(/<span[^>]*data-audit="agents(?:Role)?Glyph"[\s\S]*?<\/span>/g)].map(
      (m) =>
        `${/data-icon="([^"]*)"/.exec(m[0])?.[1] ?? ''}|${/Spin/.test(m[0]) ? 'spin' : 'still'}`,
    ),
    unclassed: unclassed(html),
  }
}

/* -------------------------------------------------------------------- markup scratching */

/** Each button's accessible name: `aria-label` when it has one, else its text. */
function labels(html: string): string[] {
  return [...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].map(
    (m) => /aria-label="([^"]*)"/.exec(m[1] ?? '')?.[1] ?? text(m[2] ?? ''),
  )
}

/** The `data-run` of every run row that holds a control with this audit hook. */
function rowsHolding(html: string, hook: string): string[] {
  return all(html, 'agentsRow')
    .filter((row) => row.includes(`data-audit="${hook}"`))
    .map((row) => attr(row, 'data-run'))
}

/**
 * Every element carrying `data-audit="<hook>"`, as its own outer HTML.
 *
 * Extents are found by counting opening and closing tags **of the same name**, rather than by
 * slicing to the next marker: a run row contains the stale bar, and a section contains its
 * rows, so a slice-to-next-marker digest would attribute one element's buttons to another and
 * the "exactly two buttons in the bar" assertion would be measuring the whole section.
 */
function all(html: string, hook: string): string[] {
  return [...html.matchAll(new RegExp(`data-audit="${hook}"`, 'g'))].map((m) =>
    outer(html, m.index),
  )
}

function outer(html: string, at: number): string {
  const start = html.lastIndexOf('<', at)
  const name = /^<([a-zA-Z0-9]+)/.exec(html.slice(start, at + 1))?.[1]
  if (name === undefined) return ''
  const tags = new RegExp(`</?${name}\\b`, 'g')
  tags.lastIndex = start
  let depth = 0
  let m = tags.exec(html)
  while (m !== null) {
    depth += m[0].startsWith('</') ? -1 : 1
    if (depth === 0) {
      const end = html.indexOf('>', m.index)
      return html.slice(start, end === -1 ? undefined : end + 1)
    }
    m = tags.exec(html)
  }
  return html.slice(start)
}

function attr(fragment: string, name: string): string {
  return new RegExp(`${name}="([^"]*)"`).exec(fragment)?.[1] ?? ''
}

function count(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1
}

/** Markup to readable text: drop tags, collapse whitespace, decode the few entities React writes. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, ' ')
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/\s+/g, ' ')
    .trim()
}

function unclassed(html: string): number {
  /* `/?` before the close: React writes void elements as `<input …/>`, and without it every
     `<input>` on the detail form would fall out of this counter unchecked. */
  const hooked = [...html.matchAll(/<[a-z]+((?:\s+[a-z-]+(?:="[^"]*")?)*)\s*\/?>/g)].filter((m) => {
    const attrs = m[1] ?? ''
    return attrs.includes('data-audit=') && !/\sclass="[^"]*[^"\s][^"]*"/.test(attrs)
  }).length
  const poisoned = [...html.matchAll(/class="([^"]*)"/g)].filter((m) =>
    (m[1] ?? '').split(/\s+/).includes('undefined'),
  ).length
  return hooked + poisoned
}
