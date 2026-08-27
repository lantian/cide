/**
 * Renders every OpenSpec panel story to static HTML and prints one JSON digest line. (M28)
 *
 * Driven by `ui/scripts/check-openspec-render.mjs`. Not part of the app — nothing imports it, so
 * it is tree-shaken out of the real bundle. `TasksPanel/smokeEntry.tsx` is the model.
 *
 * `model.ts` is checked directly under node by `check-openspec.mjs`; what this adds is the half a
 * pure core cannot speak for:
 *
 *  - that the **absent** screen prints the path it would create *before* its button, and that the
 *    **unknown** screen prints neither. `canWrite` answering `false` is worth nothing if the
 *    markup offers Set up anyway, and that button adds a directory a teammate sees in the next
 *    pull request.
 *  - that a board cide could not read renders **zero** writing controls except Retry.
 *  - that every section is expanded by default and that collapsing one really removes its rows —
 *    a tree that draws its rows regardless would make the disclosure a lie.
 *  - that a class referenced as `styles.x` exists in the stylesheet, which neither `tsc` nor
 *    `vite build` can see.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { OpenSpecPanelView } from './OpenSpecPanel'
import { RequirementEditor } from './RequirementEditor'
import { ProposeFormView } from './ProposeForm'
import { SpecTabView } from './SpecTab'
import {
  EDITOR_STORIES,
  SPEC_STORIES,
  PROPOSE_STORIES,
  TAB_STORIES,
  type EditorStoryName,
  type SpecStoryName,
  type ProposeStoryName,
  type TabStoryName,
} from './fixture'

/** Everything between a hook's opening tag and its balanced close. */
function outer(html: string, hook: string): string[] {
  const found: string[] = []
  // `[a-z][a-z0-9]*` and not `[a-z]+`: `h1` and `h2` have digits in their names, and a tag
  // pattern that cannot match them silently reports zero elements for a heading — which is how
  // the page's own title came back `null` from a digest that had rendered it perfectly.
  const opening = new RegExp(`<([a-z][a-z0-9]*)([^>]*\\sdata-audit="${hook}")`, 'g')
  for (const match of html.matchAll(opening)) {
    const tag = match[1] ?? ''
    let depth = 0
    let index = match.index ?? 0
    const open = new RegExp(`<${tag}[\\s>]`, 'g')
    const close = new RegExp(`</${tag}>`, 'g')
    open.lastIndex = index
    close.lastIndex = index
    let cursor = index
    let guard = 0
    while (guard < 5000) {
      guard += 1
      open.lastIndex = cursor
      close.lastIndex = cursor
      const nextOpen = open.exec(html)
      const nextClose = close.exec(html)
      if (nextClose === null) break
      if (nextOpen !== null && nextOpen.index < nextClose.index) {
        depth += 1
        cursor = nextOpen.index + 1
        continue
      }
      depth -= 1
      cursor = nextClose.index + 1
      if (depth === 0) {
        found.push(html.slice(index, nextClose.index + nextClose[0].length))
        break
      }
    }
  }
  return found
}

const all = (html: string, hook: string): string[] => outer(html, hook)
const count = (html: string, hook: string): number => all(html, hook).length

/**
 * How many elements carry this hook, counted by the hook itself.
 *
 * `count` goes through [`outer`], which walks to a **balanced closing tag** — so it silently
 * answers zero for a void element. `<input>` has no `</input>`, which is how the editor's two
 * text fields came out uncounted while its two textareas did not.
 */
const hits = (html: string, hook: string): number =>
  [...html.matchAll(new RegExp(`data-audit="${hook}"`, 'g'))].length

/** The opening tag of the first element carrying this hook, void elements included. */
function tagOf(html: string, hook: string): string | null {
  const match = new RegExp(`<[a-z][a-z0-9]*[^>]*\\sdata-audit="${hook}"[^>]*>`).exec(html)
  return match === null ? null : match[0]
}
const text = (fragment: string): string =>
  fragment
    .replace(/<[^>]*>/g, '')
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, '&')
    .replace(/\s+/g, ' ')
    .trim()

function attr(fragment: string, name: string): string | null {
  const match = new RegExp(`${name}="([^"]*)"`).exec(fragment)
  return match === null ? null : (match[1] ?? null)
}

export interface SpecDigest {
  story: string
  /** The panel drew at all. */
  panel: boolean
  meta: string | null
  claim: string | null
  detail: string | null
  path: string | null
  install: string | null
  /** Does the path appear before the Set-up button in document order? */
  pathBeforeSetUp: boolean | null
  setUp: 'on' | 'off' | null
  /** The button's visible words — 'Set up OpenSpec' at rest, the present participle in flight. */
  setUpLabel: string | null
  /** Does the button carry the loader mark? Only the busy story may. */
  setUpMark: boolean
  /** The two entry points an empty board offers, by the command each types. */
  entryPoints: string[]
  retry: boolean
  /** Every `<input|textarea|select|button>` that writes. */
  writeControls: number
  sections: string[]
  sectionHints: number
  rows: string[]
  /** Each change row's action, as `<change>|<start|open>`. */
  rowActions: string[]
  /** The composer: which command it is open on, what it will send, and whether it may. */
  ask: string | null
  askPreview: string | null
  askSend: 'on' | 'off' | null
  /** Is the textarea inert? A send in flight must not be typed over. (M28) */
  askInput: 'on' | 'off' | null
  askPlaceholder: string | null
  /** The editor's own fields, so a story can assert what is live and what is inert. */
  editFields: number
  editScenarios: number
  editClauses: string[]
  editSave: 'on' | 'off' | null
  editRefusal: string | null
  editProblem: string | null
  /* The change page. */
  tabTitle: string | null
  tabStage: string | null
  tabValidity: string | null
  tabProgress: string | null
  /** The line that stands in place of the bar on an archived change. `null` when there is none. */
  tabArchived: string | null
  tabDocs: string[]
  tabSteps: string[]
  tabDeltas: string[]
  tabIssues: number
  tabStart: string | null
  /**
   * *Split work*'s label, with `|inert` appended when it is drawn disabled — `rowActions`'
   * convention, so a story where it is live keeps the digest it would have had anyway. `null`
   * when the button is not drawn at all, which is every story whose change already has a task.
   */
  tabSplit: string | null
  /**
   * Is *Split work* between the primary button and the hint?
   *
   * The hint sentence is about the primary action. A control wedged between the two reattributes
   * it, which is a rendering nothing else in this digest could show — both would still be
   * present, both still say the right words.
   */
  tabSplitPlaced: boolean
  tabFailed: string | null
  /** Every collapsible block, as `<id>|<open>`. */
  tabBlocks: string[]
  tabFind: boolean
  /** Is the find bar the first thing in the page? */
  tabFindFirst: boolean
  tabFindCount: string | null
  tabDocBodies: number
  tabDocPaths: string[]
  /** Is the "has design" marker drawn? */
  tabDesign: boolean
  /** Is the proposal drawn as prose? */
  tabProposal: boolean
  /**
   * How many top-level markdown blocks the proposal rendered to.
   *
   * `data-line` is `MarkdownPreview`'s own attribute on every top-level block, so counting it is
   * how this asserts the *renderer ran* rather than that a `<div>` exists. A wrapper that had
   * dropped its `doc` would still draw the block, the heading and the link, and every other field
   * here would be unchanged.
   */
  tabProseBlocks: number
  tabMarks: number
  tabCurrentMark: number
  /** Is the header's configuration gear drawn? (M28) */
  configure: boolean
  /** Elements carrying a hook but no class, plus any class list holding `undefined`. */
  unclassed: number
}

function digest(story: string, html: string): SpecDigest {
  const setUpFragment = tagOf(html, 'openspecSetUp')
  const pathFragment = all(html, 'openspecPath')[0] ?? null
  const setUpAt = html.indexOf('data-audit="openspecSetUp"')
  const pathAt = html.indexOf('data-audit="openspecPath"')

  const rows: string[] = []
  for (const fragment of all(html, 'openspecChangeRow')) {
    rows.push(`change|${attr(fragment, 'data-change')}|${attr(fragment, 'data-stage')}`)
  }
  for (const fragment of all(html, 'openspecSpecRow')) {
    rows.push(`spec|${attr(fragment, 'data-spec')}`)
  }

  const hooked = [...html.matchAll(/<[a-z][a-z0-9]*([^>]*\sdata-audit="[^"]*")[^>]*>/g)]
  const unclassed = hooked.filter((m) => {
    const tag = m[0]
    const cls = /class="([^"]*)"/.exec(tag)
    return cls === null || cls[1] === undefined || cls[1].includes('undefined')
  }).length

  const askBox = tagOf(html, 'openspecAsk')
  const askSend = tagOf(html, 'openspecAskSend')
  const save = tagOf(html, 'specEditSave')
  const problem = tagOf(html, 'specEditProblem')

  return {
    story,
    panel: count(html, 'openspecPanel') > 0,
    ask: askBox === null ? null : (attr(askBox, 'data-command') ?? ''),
    askPreview:
      (all(html, 'openspecAskPreview')[0] ?? null) &&
      text(all(html, 'openspecAskPreview')[0] ?? ''),
    askSend: askSend === null ? null : askSend.includes('disabled') ? 'off' : 'on',
    askInput: ((f) => (f === null ? null : f.includes('disabled') ? 'off' : 'on'))(
      tagOf(html, 'openspecAskInput'),
    ),
    askPlaceholder: tagOf(html, 'openspecAskInput') === null
      ? null
      : attr(tagOf(html, 'openspecAskInput') ?? '', 'placeholder'),
    // `hits` and not `count`: `<input>` is void, so a balanced-tag walk answers zero for it.
    editFields:
      hits(html, 'specEditName') +
      hits(html, 'specEditText') +
      hits(html, 'specEditScenarioTitle') +
      hits(html, 'specEditScenarioBody'),
    editScenarios: hits(html, 'specEditScenario'),
    editClauses: all(html, 'specEditClause').map((f) => attr(f, 'data-clause') ?? ''),
    editSave: save === null ? null : save.includes('disabled') ? 'off' : 'on',
    editRefusal:
      (all(html, 'specEditRefusal')[0] ?? null) && text(all(html, 'specEditRefusal')[0] ?? ''),
    editProblem: problem === null ? null : (attr(problem, 'data-kind') ?? ''),
    tabTitle: (all(html, 'specTabTitle')[0] ?? null) && text(all(html, 'specTabTitle')[0] ?? ''),
    tabStage: ((f) => (f === null ? null : attr(f, 'data-stage')))(tagOf(html, 'specTabStage')),
    tabValidity: ((f) => (f === null ? null : attr(f, 'data-state')))(
      tagOf(html, 'specTabValidity'),
    ),
    tabProgress: ((f) => (f === null ? null : attr(f, 'aria-valuenow')))(
      tagOf(html, 'specTabProgress'),
    ),
    tabArchived:
      (all(html, 'specTabArchived')[0] ?? null) && text(all(html, 'specTabArchived')[0] ?? ''),
    tabDocs: all(html, 'specTabDoc').map((f) => attr(f, 'data-artifact') ?? ''),
    tabSteps: all(html, 'specTabStep').map((f) => attr(f, 'data-done') ?? ''),
    tabDeltas: all(html, 'specTabDelta').map(
      (f) => `${attr(f, 'data-op')}|${attr(f, 'data-spec')}`,
    ),
    tabIssues: hits(html, 'specTabIssue'),
    tabStart: (all(html, 'specTabStart')[0] ?? null) && text(all(html, 'specTabStart')[0] ?? ''),
    tabSplit: ((f) =>
      f === undefined
        ? null
        : `${text(f)}${attr(f, 'data-disabled') === 'true' ? '|inert' : ''}`)(
      all(html, 'specTabSplit')[0],
    ),
    tabSplitPlaced:
      hits(html, 'specTabSplit') > 0 &&
      html.indexOf('data-audit="specTabStart"') < html.indexOf('data-audit="specTabSplit"') &&
      html.indexOf('data-audit="specTabSplit"') < html.indexOf('data-audit="specTabHint"'),
    tabFailed:
      (all(html, 'specTabFailed')[0] ?? null) && text(all(html, 'specTabFailed')[0] ?? ''),
    tabBlocks: all(html, 'specTabBlock').map(
      (f) => `${attr(f, 'data-block')}|${f.slice(0, f.indexOf('>')).includes(' open') ? 'open' : 'shut'}`,
    ),
    tabFind: hits(html, 'specTabFind') > 0,
    tabFindFirst:
      hits(html, 'specTabFind') > 0 &&
      html.indexOf('data-audit="specTabFind"') < html.indexOf('data-audit="specTabTitle"'),
    tabFindCount:
      (all(html, 'specTabFindCount')[0] ?? null) && text(all(html, 'specTabFindCount')[0] ?? ''),
    tabDocBodies: hits(html, 'specTabDocBody'),
    tabDocPaths: all(html, 'specTabDoc').map((f) => text(f)),
    tabDesign: hits(html, 'specTabDesign') > 0,
    tabProposal: hits(html, 'specTabProposal') > 0,
    tabProseBlocks: [...html.matchAll(/ data-line="/g)].length,
    tabMarks: hits(html, 'specTabMark'),
    tabCurrentMark: [...html.matchAll(/data-current="true"/g)].length,
    meta: (all(html, 'openspecMeta')[0] ?? null) && text(all(html, 'openspecMeta')[0] ?? ''),
    claim: (all(html, 'openspecClaim')[0] ?? null) && text(all(html, 'openspecClaim')[0] ?? ''),
    detail: (all(html, 'openspecDetail')[0] ?? null) && text(all(html, 'openspecDetail')[0] ?? ''),
    path: pathFragment === null ? null : text(pathFragment),
    install:
      (all(html, 'openspecInstall')[0] ?? null) && text(all(html, 'openspecInstall')[0] ?? ''),
    pathBeforeSetUp: setUpAt < 0 || pathAt < 0 ? null : pathAt < setUpAt,
    setUp:
      setUpFragment === null ? null : setUpFragment.includes('disabled') ? 'off' : 'on',
    // `all`, not `tagOf`: the label and the mark are *inside* the element, and `tagOf` stops at
    // the opening tag — which is how a digest of them would silently read as absent for ever.
    setUpLabel:
      (all(html, 'openspecSetUp')[0] ?? null) && text(all(html, 'openspecSetUp')[0] ?? ''),
    setUpMark: (all(html, 'openspecSetUp')[0] ?? '').includes('data-icon="loader-circle"'),
    entryPoints: [
      ...all(html, 'openspecPropose'),
      ...all(html, 'openspecExplore'),
    ].map((fragment) => attr(fragment, 'data-command') ?? ''),
    retry: count(html, 'openspecRetry') > 0,
    configure: hits(html, 'openspecConfigure') > 0,
    writeControls: [...html.matchAll(/data-write="true"/g)].length,
    sections: all(html, 'openspecSection').map(
      (fragment) => `${attr(fragment, 'data-section')}|${attr(fragment, 'data-open')}`,
    ),
    sectionHints: count(html, 'openspecSectionHint'),
    rows,
    /*
     * `|inert` when the control is drawn disabled — appended rather than always present, so the
     * enabled rows read as they always did and the story that greys them is the only one that
     * moves. An action's *inertness* is in the digest because a live-looking button that does
     * nothing is the failure this panel was reported for; see `rowAction`.
     */
    rowActions: all(html, 'openspecRowAction').map(
      (fragment) =>
        `${attr(fragment, 'data-change')}|${attr(fragment, 'data-action')}` +
        (attr(fragment, 'data-disabled') === 'true' ? '|inert' : ''),
    ),
    unclassed,
  }
}

const digests: SpecDigest[] = [
  ...(Object.keys(SPEC_STORIES) as SpecStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<OpenSpecPanelView {...SPEC_STORIES[story]} />)),
  ),
  ...(Object.keys(EDITOR_STORIES) as EditorStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<RequirementEditor {...EDITOR_STORIES[story]} />)),
  ),
  ...(Object.keys(PROPOSE_STORIES) as ProposeStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<ProposeFormView {...PROPOSE_STORIES[story]} />)),
  ),
  ...(Object.keys(TAB_STORIES) as TabStoryName[]).map((story) =>
    digest(story, renderToStaticMarkup(<SpecTabView {...TAB_STORIES[story]} />)),
  ),
]

console.log(JSON.stringify(digests))
