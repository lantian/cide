/**
 * "Open this file *at this line*" — the half of a search-result click that the editor owns.
 *
 * > *"search result click doesn't point me to found place (should open file and select the
 * > line)"*
 *
 * `SearchPanel` already computes where a hit is and hands it out as
 * `onOpenHit(path, line, column, endColumn)`. Nothing could act on the last three: the
 * buffer lives in a CodeMirror `EditorState` inside `EditorSurface`, which is unreachable
 * from the sidebar, from `App.tsx`, and from anything else above the pane tree. This module
 * is that reach — the same shape as `openBuffers.ts` and `layout/paneHosts.ts`, and for the
 * same reason: a live editor outlives its position in the React tree, so the thing that
 * holds it is a module and not a component.
 *
 * # Why the request is queued and not simply delivered
 *
 * The caller requests the reveal and opens the tab **in the same turn**, so in the case that
 * matters — a hit in a file that is not open — the editor mounts *after* the request. A
 * reveal that fired immediately against an empty registry would be dropped exactly when it
 * was wanted, and would appear to work whenever the file happened to be open already. So a
 * request with no live editor is parked, and the first `EditorSurface` that mounts for that
 * path spends it.
 *
 * A file that *is* already open gets no mount at all, which is why `requestReveal` delivers
 * to live receivers rather than parking unconditionally.
 *
 * # Why the queue is bounded twice
 *
 * A path that never opens — a hit in a file the open failed on, a project closed mid-click —
 * leaves an entry nobody will ever claim. Two bounds, because they answer different things:
 *
 * * `PENDING_LIMIT` bounds the memory. It cannot grow, whatever the caller does.
 * * `REVEAL_TTL_MS` bounds the *behaviour*. Without it, an entry parked for a file that
 *   never opened would fire minutes later when the user opens that same file by hand from
 *   the Explorer — the caret would jump somewhere they did not ask for, for a search they
 *   have forgotten. A reveal is the answer to a click, and it goes stale with the click.
 *
 * The dispatch itself is not here: `EditorSurface` owns the `EditorView`, and this module
 * stays free of `@codemirror/view` so `ui/scripts/check-editor.mjs` can compile and exercise
 * it under node. What is here is everything that can be got wrong quietly — the queue and
 * the clamp.
 */
import { EditorSelection, type SelectionRange, type Text } from '@codemirror/state'

/**
 * A place in a file, in the units every editor and every stack trace uses.
 *
 * `column`/`endColumn` are UTF-16 code units and **not** the wire's byte offsets — a hit on
 * a line with a `é` or a `漢` before the match is off by one byte per non-ASCII character,
 * which lands the caret inside the wrong word. `SearchModel.hitPosition` does that
 * conversion; this module trusts it and clamps it.
 */
export interface RevealTarget {
  /** 1-based. */
  readonly line: number
  /** 1-based, UTF-16 units. */
  readonly column: number
  /** 1-based and exclusive: the selection is `[column, endColumn)` on `line`. */
  readonly endColumn: number
}

/** Applies a reveal to one live editor. `EditorSurface` registers one per mounted buffer. */
export type RevealReceiver = (target: RevealTarget) => void

/** How long a parked request stays worth honouring. See the header. */
export const REVEAL_TTL_MS = 10_000

/**
 * How many unclaimed requests are kept.
 *
 * Eight rather than one: a click that opens a file is followed by another click a second
 * later often enough, and dropping the first would make a burst of clicks reveal only the
 * last file. Eight rather than unbounded, because nothing here can tell a request that is
 * about to be claimed from one that never will be.
 */
export const PENDING_LIMIT = 8

interface Parked {
  readonly target: RevealTarget
  /** `Date.now()` at the request, for `REVEAL_TTL_MS`. */
  readonly at: number
}

const receivers = new Map<string, Set<RevealReceiver>>()
const parked = new Map<string, Parked>()

/**
 * Ask for the caret to land on `target` in `path`.
 *
 * Applied now if an editor for that path is mounted, and parked for the next one otherwise.
 * A second request for the same path replaces the first rather than joining it: the user
 * clicked twice, and the second click is the one they meant.
 */
export function requestReveal(path: string, target: RevealTarget): void {
  const live = receivers.get(path)
  if (live !== undefined && live.size > 0) {
    // Anything parked for this path is answered by this request, whatever it said.
    parked.delete(path)
    /*
     * Every editor on that path, not one of them. A file split across two panes is two
     * buffers over the same document (see `panes/EditorPane.tsx`), and choosing between them
     * would need to know which pane has focus — which is a fact about the pane tree that this
     * module deliberately cannot see. Revealing in both is never *wrong*; revealing in the
     * one the user is not looking at is wrong half the time.
     *
     * Copied before the walk: a receiver is free to unregister itself, and mutating the set
     * under its own iterator is how the second pane silently stops being told.
     */
    for (const receive of [...live]) receive(target)
    return
  }
  park(path, { target, at: Date.now() })
}

/** Park one request, newest last, and hold the queue at `PENDING_LIMIT`. */
function park(path: string, entry: Parked): void {
  // Deleted and re-set rather than overwritten. `Map` iterates in insertion order and the
  // eviction below takes the oldest, so an in-place update would leave a freshly requested
  // path first in line to be thrown away.
  parked.delete(path)
  parked.set(path, entry)
  while (parked.size > PENDING_LIMIT) {
    const oldest = parked.keys().next().value
    if (oldest === undefined) break
    parked.delete(oldest)
  }
}

/** The whole parked record, timestamp included, so a re-park cannot refresh the deadline. */
function takeParked(path: string, now: number): Parked | null {
  const queued = parked.get(path)
  if (queued === undefined) return null
  parked.delete(path)
  if (now - queued.at > REVEAL_TTL_MS) return null
  return queued
}

/**
 * Take the request parked for `path`, or `null` when there is none worth honouring.
 *
 * Removes it either way — a request is spent by the first editor that asks for it, and an
 * expired one is spent by being looked at. `now` is a parameter so the staleness rule can be
 * checked without a fake clock.
 */
export function claimReveal(path: string, now: number = Date.now()): RevealTarget | null {
  return takeParked(path, now)?.target ?? null
}

/**
 * Register a mounted editor for `path`, and answer with the function that unregisters it.
 *
 * A disposer rather than the `unregisterBuffer(key)` pair `openBuffers.ts` uses, because the
 * key is not unique here: two panes can show one file, and `unregisterReveal(path)` from the
 * one being closed would silently disconnect the one that stays.
 *
 * Any parked request for the path is spent immediately — this is the mount the caller was
 * waiting for.
 *
 * # Why the handoff is provisional for one turn
 *
 * `main.tsx` keeps `StrictMode` on deliberately, and in development it mounts every effect
 * *twice*: setup, cleanup, setup, all synchronously inside one React commit. So the first
 * `EditorSurface` to register for a path is a throwaway whose view is destroyed a few
 * statements later, and handing it the parked request and calling the request spent left the
 * live second view with nothing. The caret did not move — in the only build a human runs
 * before shipping, for exactly the case the user reported (a hit in a file that is not open).
 * That was measured against this module, not guessed.
 *
 * So a claimed request is held rather than dropped, and put back — with its original
 * timestamp, so `REVEAL_TTL_MS` still runs from the click and not from the remount — if this
 * receiver goes away leaving no editor on that path. A microtask releases the hold: React's
 * setup/cleanup/setup runs to completion before the stack empties, so anything still held
 * when the microtask fires was handed to a view that outlived the commit.
 *
 * Re-parking *unconditionally* on the last disposer was the alternative, and it loses on the
 * module's own rule: it would replay the reveal when the user closes the tab and reopens the
 * same file by hand within the TTL, which is the surprise `REVEAL_TTL_MS` exists to prevent.
 * Both alternatives fix development; only this one leaves production behaviour alone.
 */
export function registerReveal(path: string, receive: RevealReceiver): () => void {
  const existing = receivers.get(path)
  const live = existing ?? new Set<RevealReceiver>()
  if (existing === undefined) receivers.set(path, live)
  live.add(receive)

  let provisional = takeParked(path, Date.now())
  if (provisional !== null) {
    const target = provisional.target
    // Released on the next microtask, never on a timer: a timer would keep the request
    // recoverable across real user gestures, which is the unconditional re-park above.
    queueMicrotask(() => {
      provisional = null
    })
    receive(target)
  }

  return () => {
    live.delete(receive)
    // Guarded, so a disposer called twice — or after the path was re-registered into a fresh
    // set — cannot delete somebody else's registry entry.
    if (live.size === 0 && receivers.get(path) === live) receivers.delete(path)
    // Nothing is watching this path any more, and the view that was handed the request never
    // outlived the commit that made it. The request was never shown to anyone; put it back.
    if (provisional !== null && !receivers.has(path)) {
      park(path, provisional)
      provisional = null
    }
  }
}

/** The paths with a request still worth honouring. For tests and diagnostics. */
export function pendingReveals(now: number = Date.now()): string[] {
  const paths: string[] = []
  for (const [path, queued] of parked) {
    if (now - queued.at <= REVEAL_TTL_MS) paths.push(path)
  }
  return paths
}

/** The paths with a live editor. For tests and diagnostics. */
export function revealReceivers(): string[] {
  return [...receivers.keys()]
}

/**
 * The selection a target means in this document — clamped to the line and to the document.
 *
 * The clamp is the point of this function, not a courtesy. Between the search running and
 * the click landing the file can have been edited, truncated or replaced: the agent writes
 * one, a `cargo fmt` shortens one, the buffer holds edits the search never saw. An
 * out-of-range position throws inside `doc.line`/`EditorSelection`, and that exception
 * escapes through `dispatch` into a React event handler with no error boundary above it —
 * the pane and every terminal beside it go with it. A caret on the wrong line is a
 * disappointment; a blank window is a lost session.
 *
 * Non-finite input is clamped too rather than rejected. It cannot arrive from `hitPosition`
 * today, and `NaN` fails every comparison a clamp is made of, so it would sail through one
 * written the obvious way and throw at the end of it.
 */
export function revealRange(doc: Text, target: RevealTarget): SelectionRange {
  const line = doc.line(clamp(whole(target.line, 1), 1, doc.lines))
  // `line.to` and not `line.to + 1`: a column past the end of a shortened line puts the
  // caret at the end of that line, never on the break or into the line below it.
  const anchor = clamp(line.from + whole(target.column, 1) - 1, line.from, line.to)
  // From `anchor` up, so an `endColumn` before `column` — a reversed or empty match — is an
  // empty selection at the caret rather than a backwards range.
  const head = clamp(line.from + whole(target.endColumn, 1) - 1, anchor, line.to)
  return EditorSelection.range(anchor, head)
}

function whole(value: number, fallback: number): number {
  return Number.isFinite(value) ? Math.trunc(value) : fallback
}

function clamp(value: number, lo: number, hi: number): number {
  if (value < lo) return lo
  return value > hi ? hi : value
}
