/**
 * What the Extensions panel draws, as a pure function of a snapshot. (M22)
 *
 * **Import-free on purpose**, exactly as `GitPanel/model.ts`, `ProblemsPanel/model.ts` and
 * `AgentsPanel/model.ts` are, and for the same reason: `ui/scripts/check-ext.mjs` compiles
 * it standalone with the TypeScript already in `node_modules` and executes its rules. A rule that
 * lives inside a React component is a rule no check script can compile, and this project has paid
 * for that five times.
 *
 * It is also why the wire types are restated structurally here rather than imported from
 * `@/ipc/client`: that would drag `@tauri-apps/api` into a check whose whole value is that it
 * needs neither a bundler nor a DOM. `check-ext.mjs` pins the two against each other.
 */

/** The three states a marketplace row can be in, flattened for the view. */
export type MarketState = 'ready' | 'working' | 'missing' | 'failed'

export interface MarketRowIn {
  readonly id: string
  readonly name: string
  readonly source: string
  readonly authenticated: boolean
  readonly state: { readonly kind: string; readonly error?: string; readonly what?: string }
  readonly entries: readonly EntryIn[]
  readonly problems: readonly ProblemIn[]
}

export interface EntryIn {
  readonly id: string
  readonly name: string
  readonly version: string
  readonly description: string
  readonly capabilities: readonly string[]
  readonly installed?: string | undefined
  readonly updateAvailable: boolean
  readonly unavailable?: string | undefined
}

export interface InstalledIn {
  readonly marketplace: string
  readonly extension: string
  readonly name: string
  readonly version: string
  readonly enabled: boolean
  readonly capabilities: readonly string[]
  readonly unavailable?: string | undefined
  readonly problems: readonly ProblemIn[]
}

/** One language in the resolved registry, flattened for the view. */
export interface LanguageIn {
  readonly id: string
  readonly label: string
  readonly extensions: readonly string[]
  /** `null` for a builtin, else `<marketplace>.<extension>`. */
  readonly source: string | null
  /** What it displaced, in the same spelling. */
  readonly supersedes?: string | null | undefined
}

export interface ProblemIn {
  readonly path: string
  readonly line?: number | undefined
  readonly severity: string
  readonly message: string
}

/**
 * One row of the *Languages* readout.
 *
 * The panel's answer to *"why is my `.sql` file coloured like that"*. It exists because cide
 * ships grammars of its own, so the ordinary state after installing a language extension is that
 * a builtin **lost** — and a user who cannot see that happen has no way to tell a working
 * extension from an inert one. `cide-headless ext` prints the same three columns.
 */
export interface LanguageRow {
  readonly id: string
  readonly label: string
  /** `.sql .ddl` — what a file has to be called to get this language. */
  readonly extensions: string
  /** `cide`, or the extension that contributed it. */
  readonly by: string
  /** `was cide`, when this displaced something. Absent when it displaced nothing. */
  readonly instead?: string | undefined
}

/** One row under a marketplace heading. */
export interface CatalogRow {
  readonly marketplace: string
  readonly extension: string
  readonly name: string
  readonly version: string
  readonly description: string
  readonly capabilities: readonly string[]
  /**
   * What the row's primary button says, and it is a closed set because each one is a different
   * *act*: an update copies new code, an install asks for consent, and a reinstall is what a
   * broken install offers. A single "Install" that meant all three would be a button whose effect
   * the user cannot predict.
   */
  readonly action: 'install' | 'update' | 'installed' | 'unavailable'
  readonly enabled: boolean
  /** The greyed reason, when there is one. */
  readonly note?: string | undefined
}

export interface MarketGroup {
  readonly id: string
  readonly name: string
  readonly source: string
  readonly authenticated: boolean
  readonly state: MarketState
  /** One sentence for a `failed` or `working` state; absent otherwise. */
  readonly detail?: string | undefined
  readonly rows: readonly CatalogRow[]
}

/**
 * Which rows the panel is showing.
 *
 * A closed set of *questions a user actually asks*, not a set of boolean toggles. Every one of
 * them is a sentence somebody has said out loud about an editor's extension list — "what have I
 * got", "what did I turn off", "what needs updating", "what is broken" — and each is one click.
 * Combining them (installed **and** has an update) is what the search box is for, because a
 * filter matrix is a control nobody uses correctly on the first try.
 *
 * `all` is first and is the default, so the panel opens on the catalog rather than on a view the
 * user has to undo.
 */
export type ExtFilter = 'all' | 'installed' | 'enabled' | 'disabled' | 'updates' | 'available' | 'problems'

/** Every filter, in the order the chips are drawn. */
export const EXT_FILTERS: readonly ExtFilter[] = [
  'all',
  'installed',
  'enabled',
  'disabled',
  'updates',
  'available',
  'problems',
]

/** What each chip says. Short, because seven of them share a 252px column. */
export const FILTER_LABEL: Readonly<Record<ExtFilter, string>> = {
  all: 'All',
  installed: 'Installed',
  enabled: 'Enabled',
  disabled: 'Disabled',
  updates: 'Updates',
  available: 'Not installed',
  problems: 'Problems',
}

/** What the panel is asking for. */
export interface ExtQuery {
  /** Free text. Matched against the name, the id and the description, case-insensitively. */
  readonly text: string
  readonly filter: ExtFilter
}

/** The default: everything, unfiltered. */
export const NO_QUERY: ExtQuery = { text: '', filter: 'all' }

/**
 * The panel's whole content.
 *
 * A tagged union rather than an array, on `AgentRoster`'s argument: an empty list cannot say
 * whether no marketplace is connected, one is connected and empty, or one failed to clone — and
 * those want three different screens. The first is not an edge case; it is what every user sees
 * on their first launch after this ships, and it is the state that most needs prose.
 */
export type ExtensionsModel =
  | { readonly kind: 'none'; readonly hint: string }
  | {
      readonly kind: 'ready'
      readonly groups: readonly MarketGroup[]
      /** Installed extensions whose marketplace no longer lists them. */
      readonly orphans: readonly CatalogRow[]
      /** Only the languages something contributed, or that a contribution displaced. */
      readonly languages: readonly LanguageRow[]
      readonly problems: readonly ProblemIn[]
      /**
       * How many rows each filter would show, over the **unfiltered** set.
       *
       * Counted before the query is applied, which is the whole point: a chip that read `0` only
       * because the *other* chip is selected would be a control that lies about what pressing it
       * does. It is also what lets the panel dim a chip that genuinely has nothing behind it.
       */
      readonly counts: Readonly<Record<ExtFilter, number>>
      /** What was asked for, echoed so the view need not hold it twice. */
      readonly query: ExtQuery
      /**
       * A sentence, when the query matched nothing.
       *
       * Present rather than an empty list, because an empty list under a search box says nothing
       * about *why* — and the two reasons (nothing matches this text, nothing is in this filter)
       * want different words and a different next action.
       */
      readonly empty?: string | undefined
    }

/** The sentence a first launch reads. Written here so the view has no prose of its own. */
export const NO_MARKETPLACES =
  'No marketplace is connected. A marketplace is a git repository listing extensions — '
  + 'connect one by its URL, or by a path on this machine.'

/**
 * Whether a row answers this filter.
 *
 * Exported so `check-ext.mjs` can drive it directly over a table of rows, which is a great deal
 * more legible than asserting on a filtered model six times.
 */
export function matchesFilter(row: CatalogRow, filter: ExtFilter): boolean {
  switch (filter) {
    case 'all':
      return true
    case 'installed':
      // Installed, whether or not it is switched on and whether or not it is broken. "What have I
      // got" is a question about the disk, not about what is running.
      return row.action !== 'install'
    case 'enabled':
      return row.action !== 'install' && row.enabled
    case 'disabled':
      return row.action !== 'install' && !row.enabled
    case 'updates':
      return row.action === 'update'
    case 'available':
      return row.action === 'install'
    case 'problems':
      return row.note !== undefined
  }
}

/**
 * Whether a row answers this text.
 *
 * Name, id and description, lowercased, substring. Deliberately **not** the fuzzy subsequence
 * `cide-core::commands::search` uses: that ranks a large closed set where the user is recalling a
 * command they have seen, and this is a handful of rows where they are reading. A subsequence
 * match over three or four rows mostly produces surprise — `sq` matching a description containing
 * an `s` and a `q` twelve words apart — and there is nothing to rank.
 */
export function matchesText(row: CatalogRow, text: string): boolean {
  const needle = text.trim().toLowerCase()
  if (needle === '') return true
  return (
    row.name.toLowerCase().includes(needle)
    || row.extension.toLowerCase().includes(needle)
    || row.description.toLowerCase().includes(needle)
  )
}

/** Build the model. Pure, total, and never throws on a shape it does not recognise. */
export function buildModel(
  marketplaces: readonly MarketRowIn[],
  installed: readonly InstalledIn[],
  problems: readonly ProblemIn[],
  languages: readonly LanguageIn[] = [],
  query: ExtQuery = NO_QUERY,
): ExtensionsModel {
  // Before the query, always. A search that matched nothing must not make the panel claim no
  // marketplace is connected — that screen offers a *Connect* field, and the user has connected
  // one; it is the search that is wrong, and only the search should be undone.
  if (marketplaces.length === 0 && installed.length === 0) {
    return { kind: 'none', hint: NO_MARKETPLACES }
  }

  const byRef = new Map<string, InstalledIn>()
  for (const row of installed) byRef.set(`${row.marketplace}.${row.extension}`, row)

  const all: MarketGroup[] = marketplaces.map((market) => ({
    id: market.id,
    name: market.name,
    source: market.source,
    authenticated: market.authenticated,
    state: stateOf(market.state.kind),
    detail: detailOf(market.state),
    rows: market.entries.map((entry) => {
      const key = `${market.id}.${entry.id}`
      const local = byRef.get(key)
      byRef.delete(key)
      return row(market.id, entry, local)
    }),
  }))

  // Installed, and its marketplace no longer lists it. A real state — the marketplace removed the
  // extension, or the clone is stale — and one the panel must draw rather than silently omit, or
  // the user has code on disk with no row to uninstall it from.
  const allOrphans: CatalogRow[] = [...byRef.values()].map((local) => ({
    marketplace: local.marketplace,
    extension: local.extension,
    name: local.name,
    version: local.version,
    description: '',
    capabilities: local.capabilities,
    action: 'unavailable',
    enabled: local.enabled,
    note:
      local.unavailable
      ?? 'its marketplace no longer lists it. Refresh the marketplace, or remove it.',
  }))

  // Counted over everything, before the query — see `counts`' own note. An orphan is a row like
  // any other here: it is installed, it may be disabled, and it certainly has a problem.
  const everyRow = [...all.flatMap((group) => group.rows), ...allOrphans]
  const counts = Object.fromEntries(
    EXT_FILTERS.map((filter) => [filter, everyRow.filter((r) => matchesFilter(r, filter)).length]),
  ) as Record<ExtFilter, number>

  const keep = (r: CatalogRow): boolean => matchesFilter(r, query.filter) && matchesText(r, query.text)
  const filtering = query.filter !== 'all' || query.text.trim() !== ''
  const groups: MarketGroup[] = all
    .map((group) => ({ ...group, rows: group.rows.filter(keep) }))
    // A marketplace with no matching rows is dropped *while filtering* and kept otherwise. Kept,
    // because an empty marketplace is a real state with its own sentence — "this marketplace lists
    // no extensions" — and one the user needs to see in order to disconnect it. Dropped while
    // filtering, because a heading with nothing under it is noise between the rows that matched.
    .filter((group) => !filtering || group.rows.length > 0 || group.state !== 'ready')
  const orphans = allOrphans.filter(keep)

  const matched = groups.reduce((n, group) => n + group.rows.length, 0) + orphans.length
  const empty =
    matched > 0 || !filtering
      ? undefined
      : query.text.trim() !== ''
        // The text is named, so the user can see what they are searching for when the field has
        // scrolled or they have looked away. The filter is named too when it is also narrowing,
        // because "no results" under two active constraints is ambiguous about which to relax.
        ? `Nothing matches “${query.text.trim()}”${
            query.filter === 'all' ? '' : ` in ${FILTER_LABEL[query.filter].toLowerCase()}`
          }.`
        : `Nothing is ${FILTER_LABEL[query.filter].toLowerCase()}.`

  return {
    kind: 'ready',
    groups,
    orphans,
    counts,
    query,
    empty,
    // Only the interesting ones. Listing all eleven builtins beside two contributed ones would
    // bury the answer this readout exists to give in a table nobody needs to read.
    languages: languages
      .filter((lang) => lang.source !== null || (lang.supersedes ?? null) !== null)
      .map((lang) => ({
        id: lang.id,
        label: lang.label,
        extensions: lang.extensions.map((ext) => `.${ext}`).join(' '),
        by: lang.source ?? 'cide',
        instead:
          (lang.supersedes ?? null) === null
            ? undefined
            : `was ${lang.supersedes ?? 'cide'}`,
      })),
    problems: [
      ...problems,
      ...marketplaces.flatMap((market) => market.problems),
      ...installed.flatMap((row) => row.problems),
    ],
  }
}

function row(marketplace: string, entry: EntryIn, local: InstalledIn | undefined): CatalogRow {
  const note = local?.unavailable ?? entry.unavailable
  return {
    marketplace,
    extension: entry.id,
    name: entry.name,
    version: entry.version,
    description: entry.description,
    capabilities: entry.capabilities,
    // Order matters and is the order the user cares about: a broken row is broken whatever else
    // is true of it, an update is the one thing that needs doing, and "installed" is a resting
    // state rather than a call to action.
    action:
      note !== undefined
        ? 'unavailable'
        : local === undefined
          ? 'install'
          : entry.updateAvailable
            ? 'update'
            : 'installed',
    enabled: local?.enabled ?? false,
    note,
  }
}

function stateOf(kind: string): MarketState {
  switch (kind) {
    case 'ready':
      return 'ready'
    case 'working':
      return 'working'
    case 'missing':
      return 'missing'
    default:
      // Anything cide does not recognise is reported as failed rather than assumed fine. A newer
      // build's state arriving in an older one is not a case that can happen today — both halves
      // ship together — but the direction of the guess is the point.
      return 'failed'
  }
}

function detailOf(state: {
  readonly kind: string
  readonly error?: string
  readonly what?: string
}): string | undefined {
  if (state.kind === 'failed') return state.error ?? 'it could not be read.'
  if (state.kind === 'working') return state.what ?? 'working'
  if (state.kind === 'missing') return 'not cloned yet — refresh it.'
  return undefined
}

/**
 * One line describing what an extension is asking for.
 *
 * Prose and not a list of raw capability strings, because `process:spawn` tells a user nothing and
 * *"run programs on your machine"* tells them the thing they are actually deciding. The wording is
 * `Capability::describe`'s, restated here so this module stays import-free; `check-ext.mjs`
 * pins the two against each other, because a consent sheet that described a permission
 * differently from the permission it grants would be the worst possible drift in this feature.
 */
export const CAPABILITY_PROSE: Readonly<Record<string, string>> = {
  'editor:read': 'read the files you have open',
  'editor:write': 'add highlighting, problems and an outline to files you have open',
  'fs:read': 'read files in your project',
  'git:read': 'read your git status and history',
  'process:spawn': 'run programs on your machine',
}

/**
 * The two-word version of each permission, for the panel's row tags.
 *
 * The panel has a 252px column and seven of these can appear on one row, so the *sentence* lives
 * on the extension's page — beside the Install button there — and this is its index. Every tag
 * carries `CAPABILITY_PROSE`'s wording as its tooltip, so the short form is never the only form.
 *
 * Not derived from the prose by truncation: "add highlighting, problems and an outline to files
 * you have open" truncates to "add highlighting…", which reads as a different permission from the
 * one it is.
 */
export const CAPABILITY_SHORT: Readonly<Record<string, string>> = {
  'editor:read': 'reads editors',
  'editor:write': 'annotates files',
  'fs:read': 'reads project',
  'git:read': 'reads git',
  'process:spawn': 'runs programs',
}

/**
 * Whether a permission is one the user should look twice at.
 *
 * `process:spawn` alone, and it is not a judgement call: every other capability is a *read* of
 * something already on screen or already in the repository, and this one means the manifest names
 * a binary and cide runs it against the user's project. `cide_ipc::ext::Capability` says the same
 * from the other end, and `cide_ext::manifest` refuses a server contribution without it.
 */
export function isStrongCapability(capability: string): boolean {
  return capability === 'process:spawn'
}

/** The consent sentence for a set of capabilities. Empty when it asks for nothing. */
export function consentLine(capabilities: readonly string[]): string {
  if (capabilities.length === 0) return 'This extension asks for no permissions.'
  const parts = capabilities.map((cap) => CAPABILITY_PROSE[cap] ?? cap)
  const joined =
    parts.length === 1
      ? (parts[0] ?? '')
      : `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1] ?? ''}`
  return `This extension will be able to ${joined}.`
}
