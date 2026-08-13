/**
 * Chunk arithmetic for the windowed file tree.
 *
 * The tree is flattened and windowed in Rust — `fs_tree_rows(project, offset, len)` — so the
 * renderer has to decide which slices to ask for as the virtualizer scrolls. Doing that by
 * diffing the visible range against a set of already-fetched rows means a different request
 * for every scroll position and no two requests that can be deduplicated. Aligning requests
 * to a fixed grid instead gives every visible range the same small set of chunk ids, which
 * makes "have I already asked for this?" a set membership test and makes eviction possible
 * at all.
 *
 * Pure and import-free so `ui/scripts/check-picker.mjs` can compile it standalone.
 */

/**
 * Rows per request.
 *
 * 200 matches the ≤200-row frame size the picker's injector uses, and at 24px a row that is
 * about seven screens of tree — enough that ordinary scrolling stays inside one or two
 * chunks, small enough that the first paint after opening a 100k-file repository fetches
 * 200 rows and not 100,000.
 */
export const CHUNK_ROWS = 200

/**
 * How many chunks stay resident.
 *
 * 64 × 200 = 12,800 rows. The point of the cap is the M8 criterion that the JS heap stays
 * flat regardless of repository size: without it, scrolling a large tree end to end holds
 * every row it ever passed. Sixty-four is far more than any scroll position needs and small
 * enough to bound the retained set at a few megabytes.
 */
export const CHUNK_CAP = 64

/** The chunk a row index belongs to. */
export function chunkOf(row: number, size = CHUNK_ROWS): number {
  return Math.floor(row / size)
}

/** The `[offset, len)` request one chunk stands for, clamped to `total`. */
export function chunkRequest(
  chunk: number,
  total: number,
  size = CHUNK_ROWS,
): { offset: number; len: number } {
  const offset = chunk * size
  return { offset, len: Math.max(0, Math.min(size, total - offset)) }
}

/**
 * Every chunk covering the half-open row range `[from, to)`, ascending.
 *
 * Empty when the range is empty or inverted — a virtualizer that has not measured yet
 * reports `[0, 0)`, and asking Rust for nothing is better than asking it for everything.
 */
export function chunksFor(from: number, to: number, size = CHUNK_ROWS): number[] {
  if (to <= from) return []
  const first = chunkOf(Math.max(0, from), size)
  const last = chunkOf(to - 1, size)
  const out: number[] = []
  for (let c = first; c <= last; c++) out.push(c)
  return out
}

/**
 * Which chunks to drop so that at most `cap` remain, least recently touched first.
 *
 * `order` is the touch order, oldest first. Chunks in `keep` are never dropped: those are
 * the ones currently on screen, and evicting one of those would make the tree blank the rows
 * the user is looking at and then re-request them.
 */
export function chunksToEvict(
  order: readonly number[],
  keep: ReadonlySet<number>,
  cap = CHUNK_CAP,
): number[] {
  const excess = order.length - cap
  if (excess <= 0) return []

  const out: number[] = []
  for (const chunk of order) {
    if (out.length >= excess) break
    if (keep.has(chunk)) continue
    out.push(chunk)
  }
  return out
}
