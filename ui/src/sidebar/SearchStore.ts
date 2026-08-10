/**
 * The search panel's state, and the poll loop that fills it.
 *
 * Rust owns the walk; this owns an append-only list of what it has found. Three rules:
 *
 * 1. **Append, never replace.** Every poll asks from `hits.length` and appends the page it
 *    gets, so a search that has found 4 000 hits costs one page per poll rather than 4 000
 *    rows again. `frame.offset` is checked against the current length before appending — a
 *    frame that answers a different offset is dropped rather than concatenated into a list
 *    with a gap or a duplicate in it.
 * 2. **Generations.** Every query change bumps a counter; a frame tagged with an older one is
 *    discarded. The echoed `frame.query` is checked as well, because the two catch different
 *    races — the counter catches a reply from the *previous* query, and the echo catches a
 *    reply from a query the store never issued (a second window's, over the same project's
 *    shared job).
 * 3. **Typing does not start a walk.** Keystrokes are debounced, so typing `spawn` starts one
 *    search rather than five, each cancelling the last.
 *
 * The `Hit`/`QueryLike` structural types in `SearchModel.ts` are checked here rather than
 * asserted: this module passes the generated `SearchHit` and `SearchQuery` straight into
 * `groupHits` and `sameQuery`, so a field renamed in Rust fails `tsc` at those call sites.
 */
import { create } from 'zustand'
import {
  isDegraded,
  pendingCommand,
  search as searchApi,
  type ProjectId,
  type SearchFrame,
  type SearchHit,
  type SearchQuery,
} from '@/ipc/client'
import { isNoIndex } from '@/store/fileIndex'
import { EMPTY_QUERY, sameQuery } from './SearchModel'

/**
 * How long the input sits still before a walk starts.
 *
 * 140ms is about one keystroke at a fast typing speed. Lower and `spawn` starts five walks
 * that each cancel the last; higher and the panel feels like it is waiting for permission.
 */
const DEBOUNCE_MS = 140

/** Between polls of a running search. ~12/s: faster than the eye, cheaper than a frame. */
const POLL_MS = 80

/**
 * Between polls while the project has no file index yet.
 *
 * `search_query` answers `NoIndex` until the project's walk has been claimed, which is the
 * first second or two after a project opens — the panel keeps asking rather than reporting
 * "no results" for a repository nobody has read yet. Slower than [`POLL_MS`] because nothing
 * is happening: this is a retry, not progress.
 */
const NO_INDEX_MS = 400

/** The command whose absence degrades the panel. */
const COMMAND = 'search_query'

export interface SearchState {
  project: ProjectId | null
  query: SearchQuery
  /** Every hit found so far, in walk order. All of one file's hits are contiguous. */
  hits: SearchHit[]
  total: number
  files: number
  scanned: number
  running: boolean
  truncated: boolean
  /** An unusable pattern, verbatim from the engine. Not a thrown error; see `client.ts`. */
  error: string | null
  /** `search_query` is not in this build. Drives the panel's dim notice. */
  degraded: boolean
  /** File groups the user has folded shut, by absolute path. */
  collapsed: Set<string>

  /** Point the panel at a project, or at nothing. Cancels whatever the last one was doing. */
  attach: (project: ProjectId | null) => void
  /** Change the pattern or a toggle. Debounced; restarts the search. */
  setQuery: (patch: Partial<SearchQuery>) => void
  /** Fold a file's hits away, or unfold them. */
  toggleGroup: (path: string) => void
  /** Stop the search and forget it. The panel's unmount. */
  stop: () => void
}

/*
 * Module scope rather than store state, exactly as `treeStore.ts` keeps its in-flight set
 * there: this is bookkeeping the renderer must never read, and putting it in the store would
 * make every poll a state update and so a re-render of the whole result list.
 */
let generation = 0
let timer: ReturnType<typeof setTimeout> | null = null

function halt(): void {
  generation += 1
  if (timer !== null) {
    clearTimeout(timer)
    timer = null
  }
}

function schedule(gen: number, delay: number): void {
  if (timer !== null) clearTimeout(timer)
  timer = setTimeout(() => {
    timer = null
    void pump(gen)
  }, delay)
}

/** An answer for a project whose index is not ready, and for a build with no such command. */
function idle(query: SearchQuery, running: boolean): SearchFrame {
  return {
    query,
    hits: [],
    offset: 0,
    total: 0,
    files: 0,
    scanned: 0,
    running,
    truncated: false,
    error: null,
  }
}

/**
 * One poll.
 *
 * Separated from the store so the loop is readable as a loop. It reads the state fresh at
 * both ends — before the call to build the request, and after it to decide whether the answer
 * is still wanted — because the user can type, switch project or close the panel in between.
 */
async function pump(gen: number): Promise<void> {
  const before = useSearch.getState()
  const project = before.project
  if (project === null || gen !== generation) return

  const frame = await pendingCommand(
    COMMAND,
    async () => {
      try {
        return await searchApi.query(project, before.query, before.hits.length)
      } catch (error) {
        // The walk has not started yet. Not a failure — the panel keeps asking.
        if (isNoIndex(error)) return idle(before.query, true)
        throw error
      }
    },
    idle(before.query, false),
  )

  if (gen !== generation) return
  const state = useSearch.getState()
  if (state.project !== project || !sameQuery(frame.query, state.query)) return

  useSearch.setState({
    // A frame that answers an offset other than the end of what is held is dropped: appending
    // it would leave a gap or a duplicate, and both are worse than one missed page, which the
    // next poll re-asks for anyway.
    hits: frame.offset === state.hits.length ? [...state.hits, ...frame.hits] : state.hits,
    total: frame.total,
    files: frame.files,
    scanned: frame.scanned,
    running: frame.running,
    truncated: frame.truncated,
    error: frame.error,
    degraded: isDegraded(COMMAND),
  })

  if (!frame.running) return
  // `scanned === 0` with `running` is the `NoIndex` retry above, which has nothing to report
  // yet; anything else is a live walk.
  schedule(gen, frame.scanned === 0 && frame.total === 0 ? NO_INDEX_MS : POLL_MS)
}

/**
 * The results half of the state, empty.
 *
 * A function rather than a shared constant: a constant would hand every caller the *same*
 * `Set` and the same array, which is one accidental mutation away from two projects sharing
 * a collapsed-groups set.
 */
function cleared() {
  return {
    hits: [] as SearchHit[],
    total: 0,
    files: 0,
    scanned: 0,
    running: false,
    truncated: false,
    error: null as string | null,
    collapsed: new Set<string>(),
  }
}

export const useSearch = create<SearchState>((set, get) => ({
  project: null,
  query: { ...EMPTY_QUERY },
  ...cleared(),
  degraded: false,

  /**
   * Point the panel at a project.
   *
   * No early return for "the same project", deliberately. The panel unmounts every time the
   * user picks another icon in the activity rail, and `stop` clears the results on the way
   * out — so coming back to a search box that still holds a pattern has to re-run it, or the
   * panel would show `No results` under a query it never asked.
   */
  attach(project) {
    const previous = get().project
    halt()
    // The old project's walk is stopped explicitly rather than left to finish: it is reading
    // a repository for a panel that is now pointed somewhere else.
    if (previous !== null && previous !== project) {
      void searchApi.cancel(previous).catch(() => {})
    }
    set({ project, ...cleared() })
    if (project !== null && get().query.pattern !== '') schedule(generation, 0)
  },

  setQuery(patch) {
    const query = { ...get().query, ...patch }
    if (sameQuery(query, get().query)) return
    halt()
    set({ query, ...cleared() })
    const project = get().project
    if (project === null) return
    if (query.pattern === '') {
      // An empty box is the empty state, and the walk behind the last query is now pointless.
      void searchApi.cancel(project).catch(() => {})
      return
    }
    schedule(generation, DEBOUNCE_MS)
  },

  toggleGroup(path) {
    const collapsed = new Set(get().collapsed)
    if (!collapsed.delete(path)) collapsed.add(path)
    set({ collapsed })
  },

  stop() {
    halt()
    const project = get().project
    if (project !== null) void searchApi.cancel(project).catch(() => {})
    set({ ...cleared() })
  },
}))
