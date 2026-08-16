/**
 * Speed search, as one piece of machinery for both sidebar trees.
 *
 * The **rules** are in `speedSearch.ts` (which key does what) and in `cide_fs::speed` (what
 * matches, and where). This is the mechanism that connects them: it holds the query, asks Rust
 * on every keystroke, discards a frame that no longer answers the live question, and moves the
 * tree's cursor. Nothing here is a decision, and that is deliberate — every decision in this
 * feature has to be somewhere a check script can compile, and a React hook is not that place.
 *
 * # Why one hook for two trees that agree about nothing
 *
 * The explorer addresses rows by integer index into a flattening Rust owns, holds 200-row
 * chunks of it, and handles keys on the scroller because that is its single tab stop. The
 * changes tree holds a complete `Row[]` addressed by string id and handles keys per row because
 * it uses a roving tabindex. Those differences are real and they are all in the two adapters
 * below: `search` (ask the right command) and `land` (move the cursor). Everything between —
 * the query, the in-flight request, the staleness guard, the wrap-around, the expiry — is one
 * question asked twice, and this project has already paid for answering one question twice.
 *
 * # The staleness guard, which is the sharpest hazard in the feature
 *
 * A match list describes exactly one flattening. A watcher burst, an expand or a `git status`
 * landing renumbers every index in it, and Down would then jump to a *different file* with
 * nothing on screen admitting it. Three things guard against that, and all three are needed:
 *
 *   1. every reply carries the query it answers, so a frame that lands after a newer keystroke
 *      is dropped (the arrangement `PickerFrame` already uses);
 *   2. every reply carries the row count it was computed against, so a frame computed against a
 *      tree that has since changed size is dropped;
 *   3. `revision` — whatever the caller can offer that moves when the rows move — re-issues the
 *      search rather than trusting a list that survived the change.
 *
 * (1) and (2) are cheap and total; (3) is what covers a burst that replaced a file without
 * changing the count.
 */
import { useCallback, useEffect, useRef, useState } from 'react'

import type { TreeMatch, TreeMatches } from '@/ipc/generated'
import {
  SPEED_TIMEOUT_MS,
  type SpeedMods,
  applyKey,
  expired,
  nextMatch,
  speedKey,
  speedSummary,
} from './speedSearch'

export interface SpeedSearchOptions {
  /**
   * Ask Rust which rows match. `null` when this tree cannot be searched right now — no project,
   * no rows — which arms nothing and leaves every key alone.
   */
  search: ((query: string) => Promise<TreeMatches>) | null
  /** Move the tree's cursor onto a matched row. The explorer scrolls; the changes tree focuses. */
  land: (row: number) => void
  /** Open the row the cursor is on. What Enter does before the search closes. */
  accept: () => void
  /**
   * How many rows this tree has right now.
   *
   * Compared against the count Rust walked. A frame computed against a different-sized tree
   * describes a different flattening, and its row indices name different files — so it is
   * dropped rather than drawn, and `revision` re-issues the search. Guard (2) in the header.
   */
  count: number
  /**
   * Anything whose change means the row list has moved underneath the matches.
   *
   * Compared with `Object.is`, so a number, a string or an array identity all work. See (3) in
   * the module header.
   */
  revision: unknown
}

export interface SpeedSearch {
  readonly active: boolean
  readonly query: string
  /** `3 of 17`, `no match in the expanded tree`, `first 1000 matches`. */
  readonly summary: string
  /** The span inside this row's name, or `undefined`. Threaded down as a prop; see below. */
  spanFor: (row: number) => TreeMatch | undefined
  /**
   * Offer a keystroke to the search.
   *
   * `true` means the tree must stop — the key was consumed and `preventDefault` has already
   * been called. `false` means carry on exactly as before, which covers both "not ours" and
   * "the search has ended and this key still has its ordinary job".
   */
  onKeyDown: (event: {
    key: string
    ctrlKey: boolean
    metaKey: boolean
    altKey: boolean
    shiftKey: boolean
    preventDefault: () => void
    stopPropagation: () => void
  }) => boolean
  /** Leave the search. Called by every explicit exit that is not a keystroke. */
  exit: () => void
}

interface Frame {
  readonly query: string
  readonly matches: readonly TreeMatch[]
  readonly truncated: boolean
  /** The row count the walk saw. A frame whose count no longer matches the tree is dropped. */
  readonly count: number
}

const NO_FRAME: Frame = { query: '', matches: [], truncated: false, count: -1 }

export function useSpeedSearch(options: SpeedSearchOptions): SpeedSearch {
  const [query, setQuery] = useState('')
  const [frame, setFrame] = useState<Frame>(NO_FRAME)
  const [at, setAt] = useState(-1)

  /*
   * Callbacks in refs, for the reason `EditorSurface` states: the callers rebuild these every
   * render (`land` closes over the virtualizer, `search` over the project id), and putting them
   * in the effect's dependency list would re-issue the search on every render of the panel —
   * which, in the explorer, is every scroll tick.
   */
  const opts = useRef(options)
  opts.current = options

  /** The keystroke this query came from, for the timeout. See `speedSearch.expired`. */
  const typedAt = useRef(0)
  /**
   * Which request the answer on screen belongs to.
   *
   * A monotonic counter rather than an `AbortController`: Tauri's `invoke` has no cancellation,
   * so the reply lands regardless and the only question is whether to believe it. The echoed
   * query would nearly always be enough on its own; the counter covers the case where a user
   * types `ab`, erases to `a`, and the first `a`'s reply lands after the second's.
   */
  const issue = useRef(0)

  const clear = useCallback(() => {
    issue.current += 1
    setQuery('')
    setFrame(NO_FRAME)
    setAt(-1)
  }, [])

  /**
   * Ask, then keep the answer only if it still answers the question.
   *
   * `jump` is the whole difference between the two callers, and it is not cosmetic. A keystroke
   * has nothing to land on until the round trip completes, so the reply is what scrolls the
   * tree to the first match. A **re-search** — the rows moved underneath a query that has not
   * changed — must do the opposite: the user is standing on match seven, and jumping them back
   * to match one because a watcher burst arrived would be the tree throwing away their place.
   * The first draft had one `run` and did the jump unconditionally, which made every Down key
   * bounce back to the top: landing loads a chunk, a loaded chunk is a new row cache, a new row
   * cache is a revision, and the revision re-searched and re-landed.
   */
  const run = useCallback((next: string, jump: boolean) => {
    const ask = opts.current.search
    issue.current += 1
    const mine = issue.current
    if (ask === null) return
    void ask(next).then(
      (answer) => {
        if (issue.current !== mine) return
        // The echoed query. Rust returns what it was asked, so a reply that describes an older
        // keystroke is recognisable without trusting the ordering of two round trips.
        if (answer.query !== next) return
        setFrame({
          query: answer.query,
          matches: answer.matches,
          truncated: answer.truncated,
          count: answer.count,
        })
        if (jump) {
          const first = answer.matches[0]
          setAt(first === undefined ? -1 : 0)
          if (first !== undefined) opts.current.land(first.row)
        } else {
          // Clamped, not reset. The list may have got shorter under the cursor.
          setAt((was) => (answer.matches.length === 0 ? -1 : Math.min(was, answer.matches.length - 1)))
        }
      },
      () => {
        // A refused search — no index yet, the project closing under it — leaves the query on
        // screen with no matches, which the overlay already has a sentence for. Silent because
        // there is nothing the user could do about it and a toast per keystroke would be worse
        // than the empty list.
        if (issue.current !== mine) return
        setFrame({ query: next, matches: [], truncated: false, count: -1 })
        setAt(-1)
      },
    )
  }, [])

  /*
   * The rows moved. Ask again rather than trusting indices computed against the old list.
   *
   * Skipped while there is no search, which is almost always — so the ordinary watcher burst,
   * expand and `git status` cost one `Object.is` comparison in an effect that returns. Callers
   * pass something they already subscribe to (the explorer's row cache, the changes tree's row
   * array), so arming this costs no new store subscription and therefore no new re-render — a
   * live constraint here, because `check:tree-flicker` pins the explorer at *zero* re-renders
   * for a burst that moved nothing.
   */
  const revision = options.revision
  useEffect(() => {
    if (query.length === 0) return
    run(query, false)
    // `query` is deliberately absent: extending it already runs the search, and listing it here
    // would run a second one for every keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, run])

  /*
   * The readout's timer, and *only* the readout's.
   *
   * Expiry itself is decided by comparing timestamps in `onKeyDown` below, because a background
   * webview has its timers throttled and a timer that fires late would leave a stale query
   * armed. This is what makes the box disappear when nobody is typing; `keys/gate.ts` splits
   * the same job the same way and says why.
   */
  useEffect(() => {
    if (query.length === 0) return
    const handle = setTimeout(clear, SPEED_TIMEOUT_MS)
    return () => clearTimeout(handle)
  }, [query, clear])

  const onKeyDown: SpeedSearch['onKeyDown'] = useCallback(
    (event) => {
      const mods: SpeedMods = {
        ctrl: event.ctrlKey,
        meta: event.metaKey,
        alt: event.altKey,
        shift: event.shiftKey,
      }
      /*
       * Expiry is read here, at the keystroke, and not from a timer. A query the user abandoned
       * two minutes ago must not have the next letter appended to it — but the *timer* that
       * would have cleared it may never have fired, because this window was in the background.
       */
      const live = query.length > 0 && !expired(typedAt.current, Date.now()) ? query : ''
      if (live !== query) clear()

      const action = speedKey(live, event.key, mods)
      if (action.kind === 'pass') return false

      if (action.kind === 'exitThenPass') {
        clear()
        return false
      }
      if (action.kind === 'exit' || action.kind === 'swallow') {
        clear()
        event.preventDefault()
        event.stopPropagation()
        return true
      }
      if (action.kind === 'accept') {
        clear()
        event.preventDefault()
        event.stopPropagation()
        opts.current.accept()
        return true
      }
      if (action.kind === 'move') {
        const to = nextMatch(frame.matches.length, at, action.delta)
        if (to >= 0) {
          setAt(to)
          const target = frame.matches[to]
          if (target !== undefined) opts.current.land(target.row)
        }
        event.preventDefault()
        event.stopPropagation()
        return true
      }

      // `extend` and `erase`, through the one function that decides what the query becomes.
      const next = applyKey(live, action)
      event.preventDefault()
      event.stopPropagation()
      if (next === null) {
        clear()
        return true
      }
      typedAt.current = Date.now()
      setQuery(next)
      run(next, true)
      return true
    },
    [query, at, frame, clear, run],
  )

  /*
   * A frame that no longer describes the tree is not drawn.
   *
   * The re-search above will replace it within a round trip; until then the overlay shows the
   * query with no matches rather than a highlight on a row that has moved. Dropping is the
   * conservative direction — a stale highlight is a claim, an absent one is a pause.
   */
  /*
   * A frame is drawn only while it still describes *this* question over *this* tree.
   *
   * Two independent conditions and both are needed: the query, because a reply can land after
   * a newer keystroke; and the row count, because a match list is a list of indices into one
   * flattening and a tree that has changed size is a different flattening. Neither alone is
   * enough — a watcher burst leaves the query untouched, and an erase back to a previous query
   * leaves the count untouched.
   */
  const usable =
    frame.query === query && query.length > 0 && frame.count === options.count
  const matches = usable ? frame.matches : []

  /*
   * A map, not a `find` per row.
   *
   * The explorer draws about thirty rows and the cap allows a thousand matches, so the obvious
   * linear scan is thirty thousand comparisons per render — on a component that re-renders on
   * every scroll tick, in a panel whose whole design (see `FileTree`'s subscription notes) is
   * about not doing per-row work. Built once per frame instead.
   */
  const spans = useRef(new Map<number, TreeMatch>())
  const built = useRef<readonly TreeMatch[]>([])
  if (built.current !== matches) {
    built.current = matches
    spans.current = new Map(matches.map((m) => [m.row, m]))
  }
  const table = spans.current
  const spanFor = useCallback((row: number) => table.get(row), [table])

  return {
    active: query.length > 0,
    query,
    /*
     * Empty until the first frame for *this* query lands.
     *
     * `no match in the expanded tree` is a true sentence about an empty match list and a false
     * one about a question nobody has answered yet, and printing it for the round trip's
     * duration would blink the sharpest of the three sentences on every keystroke — teaching
     * the user to ignore it, which is exactly what it must not become.
     */
    summary: usable ? speedSummary(matches.length, at, frame.truncated) : '',
    spanFor,
    onKeyDown,
    exit: clear,
  }
}
