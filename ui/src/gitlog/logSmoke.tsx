/**
 * Renders every log story to static HTML and prints a digest of what came out.
 *
 * Driven by `ui/scripts/check-log-render.mjs`, which asserts on the digest. This is not part of
 * the app: nothing imports it, so it is tree-shaken out of the real bundle.
 *
 * Why it exists, in one sentence: `pnpm build` succeeding says only that the panel *compiles*,
 * and the git panel was for a milestone a thing that compiled, mounted and drew nothing at all.
 * Rendering under node with `react-dom/server` is the cheapest honest answer to "does it paint",
 * and it is the only place the multi-root strip, the budget footer and the graph-is-off sentence
 * are looked at at all — none of them appear in the single-root repository this is developed in.
 *
 * `LogView` is pure, so there is nothing to stub under it: no store, no IPC, no `document`. The
 * check still installs a `window` and a `location`, because `react-dom/server` is external to the
 * bundle and the module graph around it is not something this file controls.
 */
import { renderToStaticMarkup } from 'react-dom/server'
import { LOG_STORIES, storyPair, storyView, type LogStory, type LogStoryName } from './fixture'
import { ChangedFileList } from './LogView'
import { LogView, RangeDetails } from './LogView'

/** What the check script asserts on. Strings and counts, so a failure prints something readable. */
export interface LogDigest {
  story: LogStoryName
  rows: number
  /** `shortOid|subject|author|date` per row, in draw order. */
  cells: string[]
  /** Directory headings in the details pane, grouped and flat. */
  fileDirs?: string[]
  flatFileDirs?: string[]
  /** Every file row's label, both ways round. */
  groupedLabels?: string[]
  flatLabels?: string[]
  logColumns?: string | null
  dirExpanded?: (string | undefined)[]
  rowIndents?: string[]
  foldedLabels?: string[]
  foldedDirs?: string[]
  foldedExpanded?: (string | undefined)[]
  fileIcons?: number
  dirIcons?: number
  selectedRow?: string | null
  treeToggle?: boolean
  treeTogglePressed?: boolean
  flatTogglePressed?: boolean
  /** `kind:name` for every ref chip drawn, and how many classes each carries. */
  chips: string[]
  chipClasses: number[]
  /**
   * The element each chip was drawn as, in the same order — `button` or `span`. (M21)
   *
   * The visible half of `logModel::chipTarget`: a chip that can re-root the walk is a control and
   * a chip that cannot is text, and which one it is has to be readable off the markup because
   * nothing else distinguishes them. A `<span>` that should have been a `<button>` is a feature
   * that silently does not exist; a `<button>` that should have been a `<span>` is a control that
   * hovers and then declines to act.
   */
  chipTags: string[]
  /** Each chip's `title`, in the same order — the refname, then the sentence about the gesture. */
  chipTitles: string[]
  /** Whether any `+n` chip was drawn as a control. It names a count, not a ref; it must not be. */
  chipMoreIsButton: boolean
  /** The `+n` overflow chips, verbatim. */
  chipOverflow: string[]
  /** One per drawn graph gutter. Must be `rows` or zero, never in between. */
  graphCells: number
  graphOff: string | null
  refNote: string | null
  status: string | null
  /** The label on the foot-of-list button, or `null` when there is no more to fetch. */
  more: string | null
  repoStrip: boolean
  repoChips: string[]
  /** A *Clear filters* control in the bar, and the second one beside an empty state's sentence. */
  clear: boolean
  clearEmpty: boolean
  /** Which filter controls the bar drew, in document order. */
  controls: string[]
  /** What the branch control shows — see `data-value` in `LogView.tsx`. */
  branch: string | null
  detailFiles: number
  /** The oid on the row marked `aria-selected` — the visible half of a reveal that landed. */
  selectedOid: string | null
  /** The reveal's sentence at the head of the details pane, or `null` when it says nothing. */
  revealNote: string | null
  /** Whether *Clear filters and find it* is beside it. */
  revealFind: boolean
  /**
   * `data-row-id` for every row, in draw order — `repo:oid`.
   *
   * The handle the commit context menu resolves a right-click through: `LogTab` walks up from the
   * clicked element to the nearest `[data-row-id]` and looks the row up by it. It is asserted
   * here rather than trusted because it is invisible — no pixel changes when it is missing, and
   * what breaks is the whole menu, silently, on every row.
   */
  rowIds: string[]
  /** Every row marked `aria-selected` — two of them while a comparison is on screen. (M20) */
  selectedOids: string[]
  /** The `Comparing … ` line at the head of the details pane, or `null`. */
  rangeHeader: string | null
  /** Whether the ⇄ Swap control is beside it. */
  rangeSwap: boolean
  /** The two endpoints' summaries, old → new. */
  rangeSummaries: string | null
  /** The file count, or the sentence that replaces it when there is nothing to count. */
  rangeCount: string | null
  /** Each changed-file row's path, with its counts taken off. */
  rangeFiles: string[]
  /** Each row's `+12 −3`, in the same order. */
  rangeCounts: string[]
  /** The capped-list sentence, or `null`. */
  rangeTruncated: string | null
}

/*
 * The callbacks are the identity function's cousins: this render never fires an event, and a
 * component that needed one to *paint* would be a component holding state it should not have.
 * They are here because the props are required, which is itself the check that they stay
 * required — an optional handler is how a row quietly stops being clickable.
 */
const noop = () => {}

function render(story: LogStory, asTree = true, collapsed?: ReadonlySet<string>): string {
  return renderToStaticMarkup(
    <LogView
      {...storyView(story)}
      onSelect={noop}
      onMore={noop}
      onFilter={noop}
      onRefresh={noop}
      onFindCommit={noop}
      details={<DetailsPane story={story} asTree={asTree} collapsed={collapsed} />}
    />,
  )
}

/**
 * Which pane the story is in, decided the way `LogTab` decides it: by whether a pair exists.
 *
 * Not by `story.range !== null`. The range arrives a round trip after the selection does, and the
 * pane has to be the compare pane for that whole interval — showing the commit's own files under
 * a header that is about to be replaced is the flash this arrangement exists to prevent. Deciding
 * on the selection is also what makes the two states mutually exclusive rather than ranked.
 */
function DetailsPane({
  story,
  asTree,
  collapsed,
}: {
  story: LogStory
  asTree: boolean
  collapsed?: ReadonlySet<string> | undefined
}) {
  const pair = storyPair(story)
  if (pair === null) return <Details story={story} asTree={asTree} collapsed={collapsed} />
  return (
    <RangeDetails
      older={pair.older}
      newer={pair.newer}
      range={story.range}
      note={null}
      onSwap={noop}
      asTree={asTree}
      theme="dark"
      selectedFile={story.range?.files[0]?.path ?? null}
      onSelectFile={noop}
      collapsedDirs={collapsed}
      onToggleDir={noop}
      onToggleTree={noop}
      onOpenFile={noop}
    />
  )
}

/**
 * The **single-commit** details pane, in the shape `LogTab` builds it.
 *
 * Duplicated rather than imported, because `LogTab.tsx` is the wiring half — it imports the IPC
 * client, which touches `window` at import time — and pulling it in here would make this render
 * about the module graph rather than about the markup. What is asserted is only the file count,
 * which is the one thing a story can be wrong about in a way `pnpm build` would not notice.
 *
 * Its two-revision sibling is **not** duplicated: `RangeDetails` lives in `LogView.tsx`, which is
 * the pure half and is imported above. That asymmetry is the point of putting it there — a
 * compare header assembled in the wiring half would be a header this render could not see, and
 * "the swap control is present" is exactly the claim that stays true until a conditional above it
 * changes.
 */
/**
 * The single commit's file list.
 *
 * The **real** `ChangedFileList` and not a stand-in, which it used to be. That mattered the
 * moment the list learned to group: a hand-rolled `.map` here rendered rows that always matched
 * whatever the fixture said and could not disagree with the component, so the grouping, the
 * indent and the rename-across-directories label had no coverage at all. `LogTab`'s own
 * `Details` still cannot be rendered here — it reaches the IPC client and the workspace store —
 * but everything below its summary line now can.
 */
function Details({
  story,
  asTree,
  collapsed,
}: {
  story: LogStory
  asTree: boolean
  collapsed?: ReadonlySet<string> | undefined
}) {
  if (story.detail === null) return null
  return (
    <ChangedFileList
      files={story.detail.files.map((f) => ({
        path: f.path,
        oldPath: f.oldPath,
        counts: null,
      }))}
      asTree={asTree}
      /* A fixed theme, so the digest is stable. The icon URLs differ between light and dark and
         a check that read the ambient one would change its answer with the developer's setting. */
      theme="dark"
      selected={story.detail.files[0]?.path ?? null}
      onSelect={noop}
      collapsed={collapsed}
      onToggleDir={noop}
      summary={`${story.detail.total.files} files`}
      onToggleTree={noop}
      onOpen={noop}
      titleFor={(f) => f.path}
    />
  )
}

/** A directory heading's name, without the ▸/▾ disclosure mark or the folded count. */
function dirLabel(fragment: string): string {
  return (/data-audit="logFileDirLabel"[^>]*>([^<]*)</.exec(fragment)?.[1] ?? '').trim()
}

/** Tag-stripped, entity-decoded text of one markup fragment. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .trim()
}

/**
 * The text of every element carrying `data-audit="<name>"`.
 *
 * Matched to the next closing tag of the same *kind*, which is safe here only because every one
 * of these elements holds spans and nothing nests an element of its own kind inside itself: a
 * row is a `div` of spans, a chip is a childless span. Anything more general would need a parser,
 * and a parser in a check script is a second implementation of the thing under test.
 */
function all(html: string, name: string, tag: string): string[] {
  const re = new RegExp(`<${tag}[^>]*data-audit="${name}"[^>]*>([\\s\\S]*?)</${tag}>`, 'g')
  return [...html.matchAll(re)].map((m) => m[1] ?? '')
}

function one(html: string, name: string, tag: string): string | null {
  const found = all(html, name, tag)
  return found.length === 0 ? null : (found[0] ?? null)
}

/**
 * The short oid on the row marked `aria-selected="true"`, or `null` when no row is.
 *
 * `aria-selected` and not the active class: class names are hashed by the CSS-modules transform,
 * so the attribute is both the stable thing to read and the one a screen reader gets. A row holds
 * spans and never another `div`, so the first `</div>` after its opening tag is its own — the
 * same assumption `all()` documents, and the reason a parser is not needed here.
 */
function selectedOidOf(html: string): string | null {
  const row = /<div[^>]*aria-selected="true"[^>]*>([\s\S]*?)<\/div>/.exec(html)?.[1]
  if (row === undefined) return null
  return mapText(one(row, 'logOid', 'span'))
}

/**
 * Every row marked `aria-selected="true"`, by short oid. (M20)
 *
 * A comparison selects two rows and the list must say so: the pane beside it is showing the range
 * between them, and highlighting one end of what it is showing would be a list that is half
 * wrong. `selectedOidOf` above answers the *reveal's* question — which single row was scrolled to
 * — and the two are kept apart because a reveal that started marking two rows and a comparison
 * that stopped marking two are different bugs.
 */
function selectedOidsOf(html: string): string[] {
  return [...html.matchAll(/<div[^>]*aria-selected="true"[^>]*>([\s\S]*?)<\/div>/g)]
    .map((m) => mapText(one(m[1] ?? '', 'logOid', 'span')))
    .filter((oid): oid is string => oid !== null)
}

/** A changed-file row's own text, with the `+12 −3` span taken off the end. */
function fileLabel(fragment: string): string {
  return text(fragment.replace(/<span[^>]*data-audit="logFileCounts"[\s\S]*?<\/span>/g, ''))
}

/** How many class tokens an element carries — the chip's kind class is the second one. */
function classCount(fragment: string): number {
  const cls = /class="([^"]*)"/.exec(fragment)?.[1] ?? ''
  return cls.split(/\s+/).filter((token) => token !== '').length
}

const digests: LogDigest[] = LOG_STORIES.map((story) => {
  const html = render(story)
  /*
   * The same story drawn flat, so the check can compare the two arrangements of one commit.
   *
   * Both readings from one fixture is the point: the claim worth asserting is not "grouping
   * produces some rows" but "grouping and flattening show the **same files**, differently". A
   * separate flat fixture could satisfy the first and quietly disagree about the second.
   */
  const flatHtml = render(story, false)
  /*
   * And once more with the first directory folded, so the check can see a fold actually hide
   * rows. A boolean prop asserted through the type system proves the prop exists; only a render
   * proves it removes anything.
   */
  const firstDir = (story.range?.files ?? story.detail?.files ?? [])
    .map((f) => f.path.slice(0, f.path.lastIndexOf('/')))
    .find((d) => d !== '')
  const foldedHtml = render(story, true, firstDir === undefined ? undefined : new Set([firstDir]))
  const rows = all(html, 'logRow', 'div')
  /*
   * The chip's opening tag, kept unstripped so its class list and its element name can be read.
   *
   * `span|button` and not `span`, since M21: a chip that re-roots the walk is a `<button>`. The
   * alternation is what keeps the *pre-existing* assertions — order, `data-kind`, two class
   * tokens — asserting about every chip rather than quietly narrowing to the inert ones, which is
   * the way a check goes green by measuring less.
   */
  const chipTags = [...html.matchAll(/<(span|button)[^>]*data-audit="logChip"[^>]*>/g)]
  return {
    story: story.name,
    rows: rows.length,
    cells: rows.map((rowHtml) =>
      [
        one(rowHtml, 'logOid', 'span') ?? '',
        one(rowHtml, 'logSubject', 'span') ?? '',
        one(rowHtml, 'logRowAuthor', 'span') ?? '',
        one(rowHtml, 'logWhen', 'span') ?? '',
      ]
        .map(text)
        .join('|'),
    ),
    // `\1` closes with whatever opened, so a chip cannot be counted twice or matched across its
    // neighbour when a row draws one of each element.
    chips: [
      ...html.matchAll(
        /<(span|button)[^>]*data-audit="logChip"[^>]*data-kind="(\w+)"[^>]*>([^<]*)<\/\1>/g,
      ),
    ].map((m) => `${m[2]}:${m[3]}`),
    chipClasses: chipTags.map((m) => classCount(m[0] ?? '')),
    chipTags: chipTags.map((m) => m[1] ?? ''),
    chipTitles: chipTags.map((m) => /title="([^"]*)"/.exec(m[0] ?? '')?.[1] ?? ''),
    chipMoreIsButton: /<button[^>]*data-audit="logChipMore"/.test(html),
    chipOverflow: all(html, 'logChipMore', 'span').map(text),
    graphCells: all(html, 'logGraph', 'span').length,
    graphOff: mapText(one(html, 'logGraphOff', 'div')),
    refNote: mapText(one(html, 'logRefNote', 'div')),
    status: mapText(one(html, 'logStatusText', 'span')),
    more: mapText(one(html, 'logMore', 'button')),
    repoStrip: all(html, 'logRepo', 'span').length > 0,
    repoChips: all(html, 'logRepo', 'span').map(text),
    clear: /data-audit="logClear"/.test(html),
    clearEmpty: /data-audit="logClearEmpty"/.test(html),
    controls: [...html.matchAll(/data-audit="(logBranch|logAuthor|logText|logRefresh)"/g)].map(
      (m) => m[1] ?? '',
    ),
    branch: /data-audit="logBranch"[^>]*data-value="([^"]*)"/.exec(html)?.[1] ?? null,
    detailFiles: all(html, 'logFile', 'button').length,
    /* Directory headings, grouped and flat. The flat reading must have none — that is the
       difference between the two arrangements stated as a fact rather than as a screenshot. */
    fileDirs: all(html, 'logFileDir', 'button').map(dirLabel),
    flatFileDirs: all(flatHtml, 'logFileDir', 'button').map(dirLabel),
    /* The same files both ways. Labels differ — a basename under a heading, a whole path
       without one — so the check compares the *sets* these resolve to, not the strings. */
    groupedLabels: all(html, 'logFile', 'button').map(fileLabel),
    flatLabels: all(flatHtml, 'logFile', 'button').map(fileLabel),
    /* Present exactly when there is something to arrange. */
    /* One icon per file row plus one per directory heading. A row that lost its icon still
       renders its text, so nothing else would notice. */
    /* Anchored on the row's audit hook and a following `<img>`, not on the `<img>` being the
       first child: a file row now leads with an empty twisty span, which is its indentation. */
    /*
     * An `<img>` reached from a row's hook without crossing another hook on the way.
     *
     * These were fixed character windows — 1500 and 200 — and the 200 broke the day the folder
     * heading's twisty stopped being the character `▾` and became a drawn mark, which is about
     * 250 characters of `<svg>` sitting between the hook and the icon. The window said "0 folder
     * icons" for markup that had two, which is the worst kind of wrong: a *fewer-icons* failure
     * from a change that added something.
     *
     * Bounded by the markup instead of by a character count. `(?:(?!data-audit=)[\s\S])*?`
     * refuses to cross the next hook, so the match is "the icon belonging to this row" by
     * construction and stays true however much is drawn inside it.
     */
    fileIcons: (html.match(/data-audit="logFile"(?:(?!data-audit=)[\s\S])*?<img/g) ?? []).length,
    dirIcons: (html.match(/data-audit="logFileDir"(?:(?!data-audit=)[\s\S])*?<img/g) ?? []).length,
    /* Every heading is a disclosure and reports whether it is open. A folded directory whose
       `aria-expanded` never changes is a control that does nothing for anyone not looking at
       the twisty. */
    dirExpanded: [...html.matchAll(/data-audit="logFileDir"[^>]*aria-expanded="([a-z]+)"/g)].map(
      (m) => m[1],
    ),
    /* The left margin on each file row's twisty slot, in draw order — which IS the indentation.
       A nested file with the same margin as a root-level one is the flat list this was reported
       as twice. */
    rowIndents: [...html.matchAll(/data-audit="logFile"[\s\S]{0,1500}?margin-left:([^;"]*)/g)].map(
      (m) => (m[1] ?? '').trim(),
    ),
    /* The same story with the first directory folded: its files are gone, its count is drawn,
       and everything under every other heading is untouched. */
    foldedLabels: all(foldedHtml, 'logFile', 'button').map(fileLabel),
    foldedDirs: all(foldedHtml, 'logFileDir', 'button').map(dirLabel),
    foldedExpanded: [
      ...foldedHtml.matchAll(/data-audit="logFileDir"[^>]*aria-expanded="([a-z]+)"/g),
    ].map((m) => m[1]),
    /* Which row is marked selected, by label. `aria-current` is the accessible half of the
       highlight — a CSS-only selection is a selection nobody is told about. */
    selectedRow:
      /data-audit="logFile"[^>]*aria-current="true"[\s\S]*?data-audit="logFileLabel"[^>]*>([^<]*)</
        .exec(html)?.[1]
        ?.trim() ?? null,
    /* The grid the divider produces. A fixed `--w-log-details` for years, so nothing noticed
       that `log_split` was stored and never read. */
    logColumns: /data-audit="log"[^>]*grid-template-columns:([^;"]*)/.exec(html)?.[1]?.trim() ?? null,
    treeToggle: /data-audit="logTreeToggle"/.test(html),
    treeTogglePressed: /data-audit="logTreeToggle"[^>]*aria-pressed="true"/.test(html),
    flatTogglePressed: /data-audit="logTreeToggle"[^>]*aria-pressed="true"/.test(flatHtml),
    /*
     * Which row the reveal landed on.
     *
     * Read from `aria-selected="true"` rather than from a class, because the class name is hashed
     * by the CSS-modules transform and the attribute is the part a screen reader — and this check
     * — can actually read. Matched to the end of the row so the oid span inside it is in range;
     * `all()` cannot help here, since it keys on `data-audit` and every row carries the same one.
     */
    selectedOid: selectedOidOf(html),
    revealNote: mapText(one(html, 'logRevealNote', 'div')),
    revealFind: /data-audit="logRevealFind"/.test(html),
    /*
     * Read off the row's own opening tag, so a `data-row-id` that drifted onto some inner span
     * would not satisfy it: `closest('[data-row-id]')` from a click on the subject has to reach
     * the row, and an attribute on a child would resolve to the wrong thing or to nothing.
     */
    rowIds: [
      ...html.matchAll(/<div[^>]*data-audit="logRow"[^>]*data-row-id="([^"]*)"[^>]*>/g),
    ].map((m) => m[1] ?? ''),
    selectedOids: selectedOidsOf(html),
    rangeHeader: mapText(one(html, 'logRangeHeader', 'strong')),
    rangeSwap: /data-audit="logRangeSwap"/.test(html),
    rangeSummaries: mapText(one(html, 'logRangeSummaries', 'div')),
    /* `logFileSummary` since M21: the count moved into the shared list's header, where it sits
       beside the grouping toggle. It says the same thing and is still the only place the pane
       states how many files it is showing. */
    rangeCount: mapText(one(html, 'logFileSummary', 'span')),
    /*
     * The range's rows are `<button>`s and the single-commit story's are `<div>`s, because this
     * file duplicates that pane and imports this one. That is why `detailFiles` above and these
     * two cannot be one field: they are read off two different tags on purpose, and a story that
     * drew the wrong pane produces zero in one of them rather than a plausible number in both.
     */
    rangeFiles: all(html, 'logFile', 'button').map(fileLabel),
    rangeCounts: all(html, 'logFileCounts', 'span').map(text),
    rangeTruncated: mapText(one(html, 'logRangeTruncated', 'div')),
  }
})

function mapText(fragment: string | null): string | null {
  return fragment === null ? null : text(fragment)
}

console.log(JSON.stringify(digests))
