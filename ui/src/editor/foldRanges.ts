/**
 * Every foldable range in a document, found by reading it. (M19)
 *
 * # Why a scanner and not a syntax tree
 *
 * CodeMirror's usual folding is `foldNodeProp` over a Lezer parse tree, and there is no parse
 * tree here. `streamGrammar.ts` explains the decision that rules one out — every language in
 * this editor is a `StreamLanguage` over a data table, chosen against `@codemirror/lang-*`
 * (50-150 KB of generated parser tables per language, eleven more dependencies) and against
 * `web-tree-sitter` (~1 MB per grammar). That header names folding as the price:
 *
 * > What is knowingly given up: anything that needs structure. […] there is no folding or
 * > indentation beyond the bracket heuristic.
 *
 * This module is that price being paid rather than avoided. It is the bracket heuristic,
 * written out with enough of a tokenizer to know that the `{` in `println!("{}")` is not a
 * block — which is the whole difference between folding that works and folding that
 * mysteriously swallows the rest of a file.
 *
 * # Why it imports nothing
 *
 * `ui/scripts/check-editor.mjs` compiles it alone with the `tsc` in `node_modules` and drives
 * it as a truth table, the way `autosave.ts`, `position.ts` and `codeIntelGate.ts` are driven.
 * A fold that starts one line off is invisible in a screenshot and obvious in an assertion, and
 * the alternative — this logic inside the `foldService` closure in `folding.ts`, which needs an
 * `EditorState` — is the one place in this codebase no check script can reach.
 *
 * The document arrives as [`DocLines`], a structural interface CodeMirror's `Text` already
 * satisfies. Same trick as `openBuffers.ts`'s `WorkspaceFocus`: describe the two methods that
 * are actually used, rather than importing the class they belong to.
 */

/**
 * The two things this module asks of a document.
 *
 * `Text` from `@codemirror/state` satisfies this structurally, and so does a plain object built
 * from an array of strings — which is what the check script hands it.
 */
export interface DocLines {
  /** Line count. At least 1: an empty document is one empty line. */
  readonly lines: number
  /** 1-based. `from`/`to` are document offsets; `text` excludes the line break. */
  line(n: number): { readonly from: number; readonly to: number; readonly text: string }
}

/** Where a foldable range came from. Carried so the recursive commands can reason about it. */
export type FoldKind = 'bracket' | 'indent' | 'heading' | 'region' | 'fence'

/**
 * One foldable range.
 *
 * `from` is the offset *after* the thing that opens the block and `to` the offset *of* the
 * thing that closes it, so a collapsed function reads `fn main() {…}` rather than `fn main() …`.
 * That is `foldInside`'s convention in `@codemirror/language`, and matching it means the
 * placeholder needs no special casing.
 */
export interface FoldRange {
  /** 1-based, and the line that keeps its text when the range is folded. */
  readonly startLine: number
  /** 1-based, and the last line the fold swallows. */
  readonly endLine: number
  readonly from: number
  readonly to: number
  readonly kind: FoldKind
}

/**
 * What one language needs for the scan to be right.
 *
 * The first six fields **mirror `GrammarSpec` exactly** and must never disagree with it — a
 * fold spec that thinks `#` opens a comment in Rust would treat every `#[derive(…)]` as a dead
 * line. They are not derived from the grammar at runtime because the grammar arrives through a
 * dynamic `import()` and fold restore has to run in the editor's mount dispatch; see
 * `languages.ts`. `check-editor.mjs` asserts the agreement field by field instead, which turns
 * the drift into a build failure rather than a wrong fold.
 */
export interface FoldSpec {
  /** Mirrors `GrammarSpec.lineComment`. */
  readonly lineComment?: string
  /** Mirrors `GrammarSpec.blockComment`. */
  readonly blockComment?: readonly [string, string]
  /** Mirrors `GrammarSpec.nestedComments`. */
  readonly nestedComments?: boolean
  /** Mirrors `GrammarSpec.quotes`. Defaults to `"` and `'`, as the grammar does. */
  readonly quotes?: string
  /** Mirrors `GrammarSpec.escapes`. Defaults to true, as the grammar does. */
  readonly escapes?: boolean
  /** Mirrors `GrammarSpec.tripleQuotes`. */
  readonly tripleQuotes?: boolean

  /**
   * The opening characters that make a block. Defaults to `{[(`; `''` disables bracket folding.
   *
   * All three and not only `{`, because a multi-line array literal, a long argument list and a
   * JSON object are all things a reader wants to collapse, and two of those languages have no
   * braces at all.
   */
  readonly brackets?: string
  /**
   * Quote characters whose strings may cross a line break.
   *
   * The default is **none**, which is the safe direction and the opposite of what the grammar
   * does. The grammar leaves a `"` open to the end of the file because a wrongly-coloured tail
   * is a cosmetic loss; here it would put every remaining `{` inside a string and silently
   * delete folding for the rest of the buffer. So a language opts in per quote character:
   * Rust's `"`, Go's and TypeScript's backtick, SQL's `'`.
   */
  readonly multilineQuotes?: string
  /**
   * Quote characters the grammar opens **in its hook** rather than through `quotes`.
   *
   * Go's and TypeScript's backtick are the whole population. Both are opened by a `stream.eat`
   * inside the language's `hook`, so `GrammarSpec.quotes` does not list them and the mirrored
   * field above must not either — but a backtick string is exactly where an unbalanced `{` lives
   * (a shell command in a Go struct tag, an HTML fragment in a template literal), so the scanner
   * has to know. Kept as a separate field so the drift check stays a plain equality.
   */
  readonly extraQuotes?: string
  /**
   * A `'` that opens a **lifetime** rather than a character literal is not a quote.
   *
   * Rust only, and it is not an optional refinement — `fn f<'a>(x: &'a str) {` is an ordinary
   * signature, and read naively the `'` before `a` opens a string that runs to the end of the
   * line. The `{` at the end of it is then inside a string, the function is not foldable, and
   * every brace after it is off by one level. In a codebase with lifetimes in it, that is most
   * of the file.
   *
   * The test is the grammar's own, in `languages/rust.ts`: `'` then an identifier **not**
   * followed by a closing `'`. The negative lookahead is the whole distinction from `'x'`.
   */
  readonly lifetimes?: boolean
  /**
   * Rust's `r"…"` and `r#"…"#`, where the closer is a quote plus as many `#` as opened it.
   *
   * Worth the fifteen lines for one language because raw strings are where a `{` most often
   * appears unbalanced — a `format!` template, an embedded shell script, a regex.
   */
  readonly rawStrings?: boolean

  /** Fold by indentation as well: Python's suites, YAML's mappings. */
  readonly indentBlocks?: boolean
  /** Fold `#` headings to the next heading of the same or higher level. Markdown. */
  readonly headingFolds?: boolean
  /** Fold ``` / ~~~ fenced blocks, and do not read headings inside one. Markdown. */
  readonly fencedBlocks?: boolean
  /** Honour `// region` / `// endregion` and `// <editor-fold>` markers. Needs `lineComment`. */
  readonly regions?: boolean
}

/**
 * Above this many lines, nothing is foldable.
 *
 * The same judgement as `HIGHLIGHT_LIMIT_BYTES` in `EditorSurface.tsx`, for the same reason: a
 * generated file of a hundred thousand lines is opened to look at, not to work in, and the
 * scan is linear but not free. Twenty thousand lines is about a millisecond here and comfortably
 * larger than the biggest hand-written file in this workspace.
 */
export const FOLD_LINE_LIMIT = 20_000

/** Mid-scan tokenizer state. Mirrors the subset of `GrammarState` that structure depends on. */
interface ScanState {
  /** Block-comment nesting depth; 0 when not in one. */
  comment: number
  /** The quote character that opened the string we are inside, or null. */
  quote: string | null
  /** Set while inside a triple-quoted string, which ends only on a matching triple. */
  triple: boolean
  /** A raw string's `#` count, or -1 when not in one. */
  hashes: number
}

interface OpenBracket {
  readonly close: string
  readonly line: number
  /** Offset just after the opener — the range's `from`. */
  readonly from: number
}

const CLOSERS: Readonly<Record<string, string>> = { '{': '}', '[': ']', '(': ')' }

/**
 * `'a`, `'static`, `'_` — but not `'x'`.
 *
 * Copied from `languages/rust.ts`'s `hook` deliberately rather than shared: that one is a
 * `StringStream.match` on a live tokenizer and this one runs over a plain string, and the two
 * modules are compiled by different checks. `check:editor` drives both against the same fixtures.
 */
const LIFETIME = /^'(?:static|_|[a-z][A-Za-z0-9_]*)(?!')/

/**
 * Every foldable range in `doc`, sorted by `from` and then widest first.
 *
 * One pass. The bracket walk, the region markers, the headings and the fences are all read off
 * the same traversal; only the indentation blocks need the second loop, and that one is a stack
 * walk rather than a search — a naive "scan forward for where the indent drops" is quadratic and
 * this file is exactly where `check-editor.mjs` has already caught two of those.
 */
export function scanFolds(doc: DocLines, spec: FoldSpec): readonly FoldRange[] {
  const total = doc.lines
  if (!(total > 0) || total > FOLD_LINE_LIMIT) return []

  const openers = spec.brackets ?? '{[('
  const quotes = (spec.quotes ?? '"\'') + (spec.extraQuotes ?? '')
  const multiline = spec.multilineQuotes ?? ''
  const escapes = spec.escapes ?? true
  const lineComment = spec.lineComment ?? ''
  const blockOpen = spec.blockComment?.[0] ?? ''
  const blockClose = spec.blockComment?.[1] ?? ''
  const region = spec.regions === true && lineComment !== '' ? regionMatchers(lineComment) : null

  const found: FoldRange[] = []
  const brackets: OpenBracket[] = []
  const regions: { line: number; from: number }[] = []
  const headings: { line: number; level: number; from: number }[] = []
  const state: ScanState = { comment: 0, quote: null, triple: false, hashes: -1 }

  /** Indent width per line, -1 for a blank one. Filled here, walked below. */
  const indents: number[] = new Array<number>(total + 1).fill(-1)
  /** The line the open fence is on, and the run of backticks or tildes that opened it. */
  let fenceLine = 0
  let fenceMark = ''

  for (let n = 1; n <= total; n++) {
    const at = doc.line(n)
    const text = at.text

    indents[n] = indentOf(text)

    // Fences before everything else: a `#` inside one is not a heading and a `{` inside one is
    // not a block. The indent above is recorded first regardless, because the heading walk trims
    // trailing *blank* lines and a fenced block is not one — a section whose whole body is a code
    // block would otherwise collapse to nothing.
    if (spec.fencedBlocks === true) {
      const mark = fenceOf(text)
      if (fenceMark !== '') {
        if (mark !== null && mark.startsWith(fenceMark)) {
          if (n > fenceLine) {
            found.push({
              startLine: fenceLine,
              endLine: n,
              from: doc.line(fenceLine).to,
              to: at.to,
              kind: 'fence',
            })
          }
          fenceMark = ''
        }
        continue
      }
      if (mark !== null) {
        fenceMark = mark
        fenceLine = n
        continue
      }
    }

    // Markers are read off the raw line rather than off a comment token, so they work the same
    // in a language whose comment syntax this spec does not describe.
    if (region !== null && state.comment === 0 && state.quote === null) {
      if (region.open.test(text)) {
        regions.push({ line: n, from: at.to })
      } else if (region.close.test(text)) {
        const opened = regions.pop()
        if (opened !== undefined && n > opened.line) {
          found.push({
            startLine: opened.line,
            endLine: n,
            from: opened.from,
            to: at.to,
            kind: 'region',
          })
        }
      }
    }

    if (spec.headingFolds === true && state.comment === 0 && state.quote === null) {
      const level = headingLevel(text)
      if (level > 0) headings.push({ line: n, level, from: at.to })
    }

    if (openers !== '' || quotes !== '' || blockOpen !== '') {
      scanLine(text, at.from, n, state, found, brackets, {
        openers,
        quotes,
        escapes,
        lineComment,
        blockOpen,
        blockClose,
        nested: spec.nestedComments === true,
        triples: spec.tripleQuotes === true,
        raw: spec.rawStrings === true,
        lifetimes: spec.lifetimes === true,
      })
    }

    // A string that reaches the end of a line closes there unless its quote is one the language
    // lets span lines. See `FoldSpec.multilineQuotes` for why this is stricter than the grammar.
    if (
      state.quote !== null &&
      !state.triple &&
      state.hashes < 0 &&
      !multiline.includes(state.quote)
    ) {
      state.quote = null
    }
  }

  if (spec.indentBlocks === true) indentFolds(doc, indents, total, found)
  if (headings.length > 0) headingFolds(doc, indents, total, headings, found)

  // Sorted by start, then widest first, so `foldAtLine` can take the first hit and the gutter
  // walks the document in order.
  found.sort((a, b) => a.from - b.from || b.to - a.to)
  return nested(found)
}

/**
 * Drop any range that *crosses* another instead of nesting inside it.
 *
 * Four independent sources feed this list — brackets, indentation, headings and explicit region
 * markers — and nothing makes them agree. A `// region` opened outside a block and closed inside
 * it produces exactly such a pair, and so does a heading that starts in the middle of one.
 *
 * They cannot both be offered. A fold is a `Decoration.replace` over `[from, to)`, and two
 * overlapping replacements are a range set with no consistent rendering: collapse both and the
 * text between them belongs to neither placeholder. There is no version of the feature where the
 * user gets a sensible answer, so the crossing range is discarded before it can be offered rather
 * than after it has been folded.
 *
 * The survivor is the one that starts earlier, and the wider one on a tie — which is what the
 * sort above already produced, so this is one linear walk with a stack. In the pathological case
 * that means a malformed region marker wins over the block it cuts across; in a well-formed file
 * the question never arises, because a region wrapping three functions starts before all of them
 * and every bracket inside it nests cleanly.
 */
function nested(sorted: readonly FoldRange[]): FoldRange[] {
  const kept: FoldRange[] = []
  const open: FoldRange[] = []
  for (const range of sorted) {
    // An exact duplicate from two sources — a bracket and an indent block that happen to cover
    // the same span — is dropped rather than kept twice. `foldAllRanges` builds every effect
    // against one state before dispatching any of them, so both copies would pass its
    // already-folded filter and land two identical `Decoration.replace` ranges on one span.
    const previous = kept[kept.length - 1]
    if (previous !== undefined && previous.from === range.from && previous.to === range.to) continue
    while (open.length > 0) {
      const top = open[open.length - 1]
      if (top !== undefined && top.to <= range.from) open.pop()
      else break
    }
    const top = open[open.length - 1]
    if (top !== undefined && range.to > top.to) continue
    kept.push(range)
    open.push(range)
  }
  return kept
}

/** The widest range that begins on `line` (1-based), or null. */
export function foldAtLine(ranges: readonly FoldRange[], line: number): FoldRange | null {
  let best: FoldRange | null = null
  for (const range of ranges) {
    if (range.startLine !== line) continue
    if (best === null || range.to - range.from > best.to - best.from) best = range
  }
  return best
}

/**
 * The innermost range that *contains* `line` without starting on it, or null.
 *
 * What Toggle fold needs. IDEA's collapse acts on the block the caret is in, not only on the
 * block whose header the caret happens to be sitting on — pressing it three lines into a
 * function body must collapse that function, which is the gesture people actually make.
 */
export function enclosing(ranges: readonly FoldRange[], line: number): FoldRange | null {
  let best: FoldRange | null = null
  for (const range of ranges) {
    if (range.startLine >= line || range.endLine < line) continue
    if (best === null || range.from > best.from) best = range
  }
  return best
}

/** Every range nested inside `outer`, including `outer` itself. The recursive commands' set. */
export function within(ranges: readonly FoldRange[], outer: FoldRange): readonly FoldRange[] {
  return ranges.filter((range) => range.from >= outer.from && range.to <= outer.to)
}

/**
 * One line of the bracket/string/comment walk.
 *
 * Split out only so the loop above stays readable; it mutates `state`, `found` and `brackets`
 * exactly as an inline block would.
 */
function scanLine(
  text: string,
  base: number,
  line: number,
  state: ScanState,
  found: FoldRange[],
  brackets: OpenBracket[],
  cfg: {
    openers: string
    quotes: string
    escapes: boolean
    lineComment: string
    blockOpen: string
    blockClose: string
    nested: boolean
    triples: boolean
    raw: boolean
    lifetimes: boolean
  },
): void {
  const length = text.length
  let i = 0

  while (i < length) {
    if (state.comment > 0) {
      if (cfg.blockClose !== '' && text.startsWith(cfg.blockClose, i)) {
        state.comment--
        i += cfg.blockClose.length
        continue
      }
      if (cfg.nested && cfg.blockOpen !== '' && text.startsWith(cfg.blockOpen, i)) {
        state.comment++
        i += cfg.blockOpen.length
        continue
      }
      i++
      continue
    }

    if (state.quote !== null) {
      i = consumeString(text, i, state, cfg.escapes)
      continue
    }

    if (cfg.lineComment !== '' && text.startsWith(cfg.lineComment, i)) return
    if (cfg.blockOpen !== '' && text.startsWith(cfg.blockOpen, i)) {
      state.comment = 1
      i += cfg.blockOpen.length
      continue
    }

    if (cfg.raw) {
      const opened = openRawString(text, i)
      if (opened !== null) {
        state.quote = '"'
        state.hashes = opened.hashes
        i = consumeString(text, opened.next, state, false)
        continue
      }
    }

    const ch = text[i] ?? ''
    // Before the quote path, because the character it is about is a quote character. Same
    // ordering, and the same regex, as the grammar's `hook`.
    if (cfg.lifetimes && ch === "'" && LIFETIME.test(text.slice(i))) {
      i += (LIFETIME.exec(text.slice(i))?.[0] ?? "'").length
      continue
    }
    if (cfg.quotes.includes(ch)) {
      // `=== true` on the `startsWith`, and the doubled quote is consumed as part of the opener:
      // `"""` must not be read as an empty string followed by a live quote. That misreading cost
      // `streamGrammar.ts` an afternoon and is recorded there.
      const isTriple = cfg.triples && text.startsWith(ch.repeat(3), i)
      state.quote = ch
      state.triple = isTriple
      state.hashes = -1
      i = consumeString(text, i + (isTriple ? 3 : 1), state, cfg.escapes)
      continue
    }

    if (cfg.openers.includes(ch)) {
      brackets.push({ close: CLOSERS[ch] ?? '', line, from: base + i + 1 })
      i++
      continue
    }

    // A closer with nothing to close is left alone; one that matches an entry further down the
    // stack pops the unmatched entries above it. Both are the shapes a half-written file takes,
    // and neither may throw or resync the rest of the document onto the wrong depth.
    if (ch === '}' || ch === ']' || ch === ')') {
      for (let k = brackets.length - 1; k >= 0; k--) {
        const open = brackets[k]
        if (open === undefined || open.close !== ch) continue
        brackets.length = k
        if (line > open.line) {
          found.push({
            startLine: open.line,
            endLine: line,
            from: open.from,
            to: base + i,
            kind: 'bracket',
          })
        }
        break
      }
    }
    i++
  }
}

/** Consume from `i` to the end of the open string or the end of the line. Returns the new `i`. */
function consumeString(text: string, i: number, state: ScanState, escapes: boolean): number {
  const quote = state.quote
  if (quote === null) return i + 1
  const length = text.length

  while (i < length) {
    if (state.hashes >= 0) {
      if (text[i] === quote) {
        let seen = 0
        while (seen < state.hashes && text[i + 1 + seen] === '#') seen++
        if (seen === state.hashes) {
          state.quote = null
          state.hashes = -1
          return i + 1 + seen
        }
      }
      i++
      continue
    }
    if (escapes && text[i] === '\\') {
      i += 2
      continue
    }
    if (state.triple) {
      if (text.startsWith(quote.repeat(3), i)) {
        state.quote = null
        state.triple = false
        return i + 3
      }
      i++
      continue
    }
    if (text[i] === quote) {
      state.quote = null
      return i + 1
    }
    i++
  }
  return i
}

/** A Rust raw-string opener at `i` — `r"`, `r#"`, `br##"` — or null. Returns the `#` count. */
function openRawString(text: string, i: number): { hashes: number; next: number } | null {
  const before = i > 0 ? (text[i - 1] ?? '') : ''
  if (/[A-Za-z0-9_]/.test(before)) return null
  let at = i
  if (text[at] === 'b') at++
  if (text[at] !== 'r') return null
  at++
  let hashes = 0
  while (text[at] === '#') {
    hashes++
    at++
  }
  if (text[at] !== '"') return null
  return { hashes, next: at + 1 }
}

/**
 * Indentation blocks, in one stack walk.
 *
 * A line opens a block when some later line is indented past it, and the block ends at the last
 * non-blank line before the indent comes back. Written as a monotonic stack rather than as a
 * forward search per line, because the forward search is O(n²) on a deeply nested file and
 * `check-editor.mjs` asserts this is linear.
 *
 * A blank line never ends a block — a Python function with a blank line in the middle of it is
 * still one function — which is why `indents` records -1 for blanks and the walk skips them.
 */
function indentFolds(doc: DocLines, indents: readonly number[], total: number, out: FoldRange[]): void {
  const stack: { line: number; indent: number }[] = []
  let previous = 0

  const close = (upTo: number, end: number): void => {
    while (stack.length > 0) {
      const top = stack[stack.length - 1]
      if (top === undefined || top.indent < upTo) break
      stack.pop()
      if (end > top.line) {
        out.push({
          startLine: top.line,
          endLine: end,
          from: doc.line(top.line).to,
          to: doc.line(end).to,
          kind: 'indent',
        })
      }
    }
  }

  for (let n = 1; n <= total; n++) {
    const indent = indents[n] ?? -1
    if (indent < 0) continue
    close(indent, previous)
    stack.push({ line: n, indent })
    previous = n
  }
  close(0, previous)
}

/**
 * Heading blocks: `##` folds to just before the next heading of level 2 or higher.
 *
 * Trailing blank lines are left out of the fold. A heading followed by a paragraph and then two
 * blank lines before the next heading should collapse to the paragraph, not to the whitespace —
 * otherwise the blank lines pile up under the collapsed row and the document does not look
 * shorter, which is the only thing folding is for.
 */
function headingFolds(
  doc: DocLines,
  indents: readonly number[],
  total: number,
  headings: readonly { line: number; level: number; from: number }[],
  out: FoldRange[],
): void {
  for (let h = 0; h < headings.length; h++) {
    const heading = headings[h]
    if (heading === undefined) continue
    let end = total
    for (let k = h + 1; k < headings.length; k++) {
      const next = headings[k]
      if (next !== undefined && next.level <= heading.level) {
        end = next.line - 1
        break
      }
    }
    while (end > heading.line && (indents[end] ?? -1) < 0) end--
    if (end <= heading.line) continue
    out.push({
      startLine: heading.line,
      endLine: end,
      from: heading.from,
      to: doc.line(end).to,
      kind: 'heading',
    })
  }
}

/** Indent width in columns, or -1 for a blank line. A tab counts as one column, as CodeMirror's does not — but only the *ordering* of two indents matters here, and no file mixes them within one block. */
function indentOf(text: string): number {
  let i = 0
  while (i < text.length) {
    const ch = text[i]
    if (ch !== ' ' && ch !== '\t') return i
    i++
  }
  return -1
}

/** The fence marker a line opens or closes with (``` or ~~~ and its length), or null. */
function fenceOf(text: string): string | null {
  const match = /^\s{0,3}(`{3,}|~{3,})/.exec(text)
  return match === null ? null : (match[1] ?? null)
}

/** `#` through `######` followed by a space. ATX only; Setext underlines are not worth the lookahead. */
function headingLevel(text: string): number {
  const match = /^(#{1,6})(\s|$)/.exec(text)
  return match === null ? 0 : (match[1] ?? '').length
}

/**
 * The two patterns that open and close an explicit region, for one comment syntax.
 *
 * Both spellings the world uses: `region`/`endregion` (Visual Studio, VS Code, Rider) with an
 * optional `#`, and `<editor-fold>` / `</editor-fold>` (IDEA's own, which is what a file written
 * in IntelliJ will contain). Case-insensitive, because `#Region` is the C# convention.
 */
function regionMatchers(lineComment: string): { open: RegExp; close: RegExp } {
  const lead = `^\\s*${escapeRegExp(lineComment)}\\s*`
  return {
    open: new RegExp(`${lead}(?:#?region\\b|<editor-fold\\b)`, 'i'),
    close: new RegExp(`${lead}(?:#?endregion\\b|</editor-fold\\s*>)`, 'i'),
  }
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\-]/g, '\\$&')
}
