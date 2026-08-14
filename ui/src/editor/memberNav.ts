/**
 * Where the caret is in a file's structure, and where the next member is.
 *
 * Three pure functions over an outline the frontend already has in hand, and that is the whole
 * design: the breadcrumb updates on every caret move — thirty times a second under a held arrow
 * key — and a round trip per move is the shape of freeze `cmd/picker.rs`'s header was written to
 * avoid. So Rust parses a file *once* per open (and once per debounced edit), the result is cached
 * per path, and these three functions answer every subsequent question locally with arithmetic.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-outline.mjs` compiles it alone with a bare `tsc` and runs the output under
 * node. That works only while the module has no imports at all — not even a type-only one through
 * the `@/*` alias, which a bare `tsc` cannot resolve. The wire types are restated structurally
 * below for the same reason `ProblemsPanel/model.ts` restates its own; `check-outline.mjs` pins
 * them against `generated.ts` so the restatement cannot drift.
 */

/** A span, in the units `RevealTarget` uses: 1-based lines, 1-based UTF-16 columns. */
export interface Span {
  readonly startLine: number
  readonly startColumn: number
  readonly endLine: number
  readonly endColumn: number
}

/**
 * The shape of `cide_ipc::Symbol` this module needs.
 *
 * Structural rather than imported — see the module header. Only the four fields the arithmetic
 * touches are named, so a new field on the Rust side cannot break this compile.
 */
export interface OutlineNode {
  readonly kind: string
  readonly name: string
  readonly container: string | null
  /** The whole declaration. What a caret is tested against. */
  readonly range: Span
  /** The name alone. Where a caret is sent. */
  readonly selection: Span
  readonly children: readonly OutlineNode[]
}

/** One row of the flattened list the structure popup draws. */
export interface FlatNode {
  readonly node: OutlineNode
  /** 0 at the top level. Drives indentation only. */
  readonly depth: number
}

/**
 * Depth-first, in document order, with each node's nesting depth.
 *
 * Document order rather than sorted: the structure popup shows a file as it is written, so an
 * `impl` block's methods sit contiguously under it. That is the sibling context IDEA's tree gives,
 * obtained without any expand/collapse state to keep.
 */
export function flattenOutline(symbols: readonly OutlineNode[], depth = 0): FlatNode[] {
  const out: FlatNode[] = []
  for (const node of symbols) {
    out.push({ node, depth })
    out.push(...flattenOutline(node.children, depth + 1))
  }
  return out
}

/** Is (line, column) inside `span`? Half-open at the end, matching the span's own convention. */
function contains(span: Span, line: number, column: number): boolean {
  if (line < span.startLine || line > span.endLine) return false
  if (line === span.startLine && column < span.startColumn) return false
  if (line === span.endLine && column > span.endColumn) return false
  return true
}

/**
 * The chain of symbols enclosing the caret, outermost first.
 *
 * `mod net` → `impl Display for Server` → `fn fmt`, which is exactly what the status bar draws
 * after the path. Empty when the caret is on a blank line between declarations, which is honest:
 * it is inside nothing.
 *
 * Tested against `range` and not `selection`, and that is the reason a symbol carries both. With
 * only the name's span, a caret anywhere in a function body — which is where a caret usually is —
 * would be inside nothing at all and the trail would be empty whenever it mattered.
 */
export function enclosingTrail(
  symbols: readonly OutlineNode[],
  line: number,
  column: number,
): OutlineNode[] {
  for (const node of symbols) {
    if (!contains(node.range, line, column)) continue
    // The first match wins and recursion goes down it: siblings cannot overlap, so there is at
    // most one containing node per level. Continuing the loop after a hit would be wasted work
    // and would let a malformed outline produce two trails.
    return [node, ...enclosingTrail(node.children, line, column)]
  }
  return []
}

/**
 * The innermost enclosing symbol, or the last one that starts above the caret.
 *
 * What the File Structure popup preselects, matching IDEA: open it with the caret in a method and
 * that method is already highlighted. The fallback matters as much as the hit — a caret on a blank
 * line between two functions is "in" neither, and preselecting the first row of the file would
 * scroll the popup away from where the user is looking.
 *
 * Returns `-1` for an empty outline, and for a caret above everything.
 */
export function enclosingIndex(
  symbols: readonly OutlineNode[],
  line: number,
  column: number,
): number {
  const flat = flattenOutline(symbols)
  const trail = enclosingTrail(symbols, line, column)
  const innermost = trail.at(-1)
  if (innermost !== undefined) {
    return flat.findIndex((row) => row.node === innermost)
  }
  // Nothing contains it. The last declaration that begins at or above the caret is the one the
  // user is reading past.
  let best = -1
  for (let i = 0; i < flat.length; i += 1) {
    const start = flat[i]!.node.range
    if (start.startLine < line || (start.startLine === line && start.startColumn <= column)) {
      best = i
    }
  }
  return best
}

/**
 * The next or previous member declaration from the caret.
 *
 * Returns the `selection` span — the name — so the caret lands on `foo` and not on the `p` of
 * `pub fn foo`. `null` at either end.
 *
 * # Clamp, never wrap
 *
 * `listKeys.ts` wraps, and says why: one Up from the top row of a picker is the last result, which
 * is how you reach the bottom of a long list without holding a key. A picker is a ring the user
 * can see all of. **A file is not.** Wrapping from the last function to the first is a jump of
 * hundreds of lines with no visual continuity, and it is the opposite of what "next" means when
 * the thing being navigated is a scroll position. IDEA clamps, and so does this.
 *
 * # Previous, from inside a body
 *
 * A caret inside a function and `direction: 'prev'` goes to **that function's own declaration**,
 * not to the one before it — IDEA's behaviour, and the one that makes the gesture useful for
 * "take me to the top of what I am in". Only a caret already on the declaration line moves past it.
 */
export function memberStep(
  symbols: readonly OutlineNode[],
  line: number,
  column: number,
  direction: 'next' | 'prev',
): Span | null {
  const flat = flattenOutline(symbols)
  if (flat.length === 0) return null

  if (direction === 'next') {
    for (const row of flat) {
      const at = row.node.selection
      if (at.startLine > line || (at.startLine === line && at.startColumn > column)) {
        return at
      }
    }
    return null
  }

  // Walking backwards, the first declaration that starts strictly above the caret is either the
  // head of the body the caret is in, or the previous sibling — which is the behaviour described
  // above, obtained without a special case for either.
  for (let i = flat.length - 1; i >= 0; i -= 1) {
    const at = flat[i]!.node.selection
    if (at.startLine < line || (at.startLine === line && at.startColumn < column)) {
      return at
    }
  }
  return null
}

/**
 * The symbol trail as strings, for the status bar.
 *
 * Separate from [`enclosingTrail`] so the bar never has to know what an `OutlineNode` is, and so
 * the comparison that decides whether to repaint is over strings — `sameTrail` in
 * `statusReadout.ts` already does exactly that for the path half.
 */
export function trailNames(
  symbols: readonly OutlineNode[],
  line: number,
  column: number,
): string[] {
  return enclosingTrail(symbols, line, column).map((node) => node.name)
}
