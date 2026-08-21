/**
 * Keeping the two halves of a split markdown pane looking at the same place. (M20)
 *
 * # Lines, not pixels, and not a ratio
 *
 * The naive version is `preview.scrollTop / previewHeight = editor.scrollTop / editorHeight`,
 * and it is wrong in the way that is most annoying to use: a fifty-line fenced code block is
 * tall in the buffer and tall in the preview, but a fifty-line paragraph is fifty lines in the
 * buffer and four wrapped lines in the preview. The ratio drifts further apart the further down
 * a document you go, which is exactly where a reader needs it to be right.
 *
 * So the mapping is anchored. Every top-level block renders with its source line on it, giving a
 * table of (source line → pixel top) pairs that is exact at each block and interpolated between
 * them. Lines are the unit for the reason `crates/cide-ipc/src/positions.rs` gives under *Why
 * lines and not pixels*: the code font size is a runtime setting and the editor wraps, so a pixel
 * offset means something different after a resize, and a line does not.
 *
 * # Why there is a latch
 *
 * Scrolling either half moves the other, and moving the other fires *its* scroll event, which
 * would move the first. That is a feedback loop, and on a trackpad it presents as the document
 * juddering and then running away. [`claimDriver`] is the whole fix: whoever moved first owns the
 * gesture for [`SYNC_HOLD_MS`], and the follower's own scroll events are ignored for that long.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-markdown.mjs` compiles it standalone and drives the arithmetic — the
 * monotonicity, the clamps at both ends, and that the latch does not oscillate. None of that is
 * visible in a screenshot, and all of it is what makes the feature feel broken when it is wrong.
 */

/** One block's position in the preview: the source line it starts on, and its pixel top. */
export interface Anchor {
  readonly line: number
  readonly top: number
}

/**
 * The geometry a mapping needs, gathered once per render rather than per scroll frame.
 *
 * `check:resize` exists because the expensive reactions to a size change have to be deferred to
 * the end of a gesture; the same rule applies to a scroll. Reading a `getBoundingClientRect` per
 * block per frame is the version of this feature that locks the app up while somebody flicks a
 * trackpad, so the caller measures on render and after a *settled* resize, and this module never
 * touches the DOM at all.
 */
export interface SyncGeometry {
  /** Ascending by `line`, and by construction also ascending by `top`. */
  readonly anchors: readonly Anchor[]
  /** The preview's full scroll height. */
  readonly contentHeight: number
  /** The preview's visible height. */
  readonly viewportHeight: number
  /** Lines in the source document, for the stretch past the last anchor. */
  readonly docLines: number
}

/** The largest `scrollTop` a container can be at. */
function maxScroll(geometry: SyncGeometry): number {
  return Math.max(0, geometry.contentHeight - geometry.viewportHeight)
}

/**
 * The index of the last anchor at or before `line`, or `-1` when `line` precedes them all.
 *
 * Binary search rather than a scan: this runs on every frame of a scroll, and a 4,000-line
 * README has a few hundred anchors.
 */
export function anchorIndexFor(anchors: readonly Anchor[], line: number): number {
  let lo = 0
  let hi = anchors.length - 1
  let found = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const anchor = anchors[mid]
    if (anchor === undefined) break
    if (anchor.line <= line) {
      found = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }
  return found
}

/**
 * Where the preview should be scrolled to, given the buffer's first visible line.
 *
 * Interpolation between anchors is linear in *source lines*, which is the best available guess
 * for a stretch whose internal structure the preview has flattened — the alternative would be a
 * per-line anchor, which is one DOM node and one measurement per line of the document.
 *
 * Past the last anchor the document's end stands in as the final pair, so scrolling into a long
 * trailing block still moves the preview rather than pinning it.
 */
export function previewTopFor(geometry: SyncGeometry, topLine: number): number {
  const { anchors } = geometry
  const limit = maxScroll(geometry)
  if (anchors.length === 0 || limit <= 0) return 0

  const index = anchorIndexFor(anchors, topLine)
  if (index < 0) return 0

  const anchor = anchors[index]
  if (anchor === undefined) return 0

  const next = anchors[index + 1]
  const nextLine = next?.line ?? Math.max(geometry.docLines + 1, anchor.line + 1)
  const nextTop = next?.top ?? geometry.contentHeight
  const span = nextLine - anchor.line
  const progress = span <= 0 ? 0 : Math.min(1, Math.max(0, (topLine - anchor.line) / span))

  return Math.min(limit, Math.max(0, anchor.top + progress * (nextTop - anchor.top)))
}

/**
 * The source line the preview's top edge is showing. The inverse of [`previewTopFor`].
 *
 * Not literally an inverse — the two round differently and the mapping is many-to-one wherever a
 * block spans several source lines — but round-tripping a value through both must not walk. The
 * check drives exactly that.
 */
export function sourceLineFor(geometry: SyncGeometry, scrollTop: number): number {
  const { anchors } = geometry
  if (anchors.length === 0) return 1

  const top = Math.min(maxScroll(geometry), Math.max(0, scrollTop))

  // Linear-search-free: the anchors ascend by `top` as well as by `line`.
  let lo = 0
  let hi = anchors.length - 1
  let index = 0
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const anchor = anchors[mid]
    if (anchor === undefined) break
    if (anchor.top <= top) {
      index = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }

  const anchor = anchors[index]
  if (anchor === undefined) return 1

  const next = anchors[index + 1]
  const nextLine = next?.line ?? Math.max(geometry.docLines + 1, anchor.line + 1)
  const nextTop = next?.top ?? geometry.contentHeight
  const span = nextTop - anchor.top
  const progress = span <= 0 ? 0 : Math.min(1, Math.max(0, (top - anchor.top) / span))

  return Math.max(1, Math.round(anchor.line + progress * (nextLine - anchor.line)))
}

/* --- the latch ------------------------------------------------------------------------------ */

/**
 * How long the half that started a gesture keeps it.
 *
 * Long enough to cover the follower's programmatic scroll and the events it fires — a smooth
 * scroll on WebKitGTK settles well inside this — and short enough that letting go of one half and
 * immediately grabbing the other feels like nothing happened. A value in the seconds would make
 * the *other* half feel dead after every scroll, which is the failure that reads as a bug.
 */
export const SYNC_HOLD_MS = 150

export type Driver = 'editor' | 'preview'

/** Who is currently driving, and when they last said so. Owned by the caller; mutated here. */
export interface SyncLatch {
  driver: Driver | null
  at: number
}

export function newLatch(): SyncLatch {
  return { driver: null, at: 0 }
}

/**
 * May `who` drive right now? Claims the gesture when the answer is yes.
 *
 * The rule is deliberately "the incumbent keeps it until it goes quiet" rather than "the last
 * event wins": a follower scrolled programmatically fires scroll events indistinguishable from a
 * user's, so last-event-wins is the feedback loop with extra steps.
 */
export function claimDriver(latch: SyncLatch, who: Driver, now: number): boolean {
  if (latch.driver !== null && latch.driver !== who && now - latch.at < SYNC_HOLD_MS) return false
  latch.driver = who
  latch.at = now
  return true
}

/** Release the gesture — used when a pane unmounts or the layout changes under it. */
export function releaseDriver(latch: SyncLatch): void {
  latch.driver = null
  latch.at = 0
}
