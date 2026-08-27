/**
 * Markdown blocks: what a *line* is. (M20)
 *
 * The other half of the parser. `inline.ts` decides what the characters inside a paragraph mean;
 * this decides where paragraphs, lists, quotes, fences and tables begin and end.
 *
 * # Two passes, because a link may be defined after it is used
 *
 * `[cide]` on line 3 can be defined by `[cide]: https://…` on line 900, so no inline text can be
 * resolved until the whole file has been read. The block pass therefore builds a tree whose
 * leaves still hold **raw source text** ([`Raw`]), collecting link reference definitions as it
 * strips them off the front of paragraphs, and a second walk turns every raw string into
 * `Inline[]` with the finished reference map. That is also what CommonMark's own reference
 * implementation does, and for the same reason.
 *
 * # This is a subset, and the boundary is drawn on purpose
 *
 * Everything a document in this repository actually uses: ATX and setext headings, thematic
 * breaks, fenced and indented code, blockquotes, bullet and ordered lists nested arbitrarily,
 * GFM tables and task items, and link reference definitions. What is deliberately absent is
 * listed in README's *Markdown preview* section — the load-bearing one being **HTML blocks**,
 * which are text here and always will be; see `types.ts`.
 *
 * # Line numbers are carried, not recomputed
 *
 * Every container strips its own marker and hands its children a `Line[]` that still knows what
 * source line each entry came from. Recomputing the number afterwards by counting newlines would
 * be wrong the moment a blockquote or a list item removed a prefix, and the number is not a
 * nicety: `scrollSync.ts` is built entirely out of it.
 *
 * Import-free apart from its own siblings, so `ui/scripts/check-markdown.mjs` can run it.
 */
import { normaliseLabel, parseInline } from './inline'
import type {
  Align,
  Block,
  Inline,
  LinkDef,
  ListItem,
  MarkdownDoc,
  MarkdownOptions,
  TableRow,
} from './types'

/** One source line, with the number it had in the file. 1-based, like everything else. */
interface Line {
  readonly text: string
  readonly no: number
}

/* --- the raw tree -------------------------------------------------------------------------- */

interface RawItem {
  line: number
  checked: boolean | null
  body: Raw[]
}

type Raw =
  | { k: 'heading'; line: number; level: number; text: string }
  | { k: 'para'; line: number; text: string }
  | { k: 'code'; line: number; lang: string | null; text: string }
  | { k: 'quote'; line: number; body: Raw[] }
  | {
      k: 'list'
      line: number
      ordered: boolean
      start: number
      tight: boolean
      items: RawItem[]
    }
  | { k: 'rule'; line: number }
  | {
      k: 'table'
      line: number
      head: string[]
      align: Align[]
      rows: { line: number; cells: string[] }[]
    }

/* --- line shapes --------------------------------------------------------------------------- */

const FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/
const ATX = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/
const RULE = /^ {0,3}(?:\*[ \t]*){3,}$|^ {0,3}(?:-[ \t]*){3,}$|^ {0,3}(?:_[ \t]*){3,}$/
const QUOTE = /^ {0,3}>/
const BULLET = /^( {0,3})([-*+])([ \t]+|$)/
const ORDERED = /^( {0,3})(\d{1,9})([.)])([ \t]+|$)/
const SETEXT = /^ {0,3}(=+|-+)[ \t]*$/
const INDENTED = /^(?: {4}|\t)/
const BLANK = /^[ \t]*$/
/** A GFM delimiter row: `| --- | :-: |`, with or without the outer pipes. */
const TABLE_DELIM = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/
/**
 * `[label]: destination "title"`.
 *
 * The label is bounded for the reason every scan in `inline.ts` is bounded: an unbounded
 * `[^\]]*` here is re-run on every line that starts with `[`, and a file of them is quadratic.
 */
const LINK_DEF = /^ {0,3}\[((?:[^\][\\]|\\.){1,512})\]:[ \t]*(\S+)(?:[ \t]+(?:"([^"]*)"|'([^']*)'|\(([^)]*)\)))?[ \t]*$/

function isBlank(text: string): boolean {
  return BLANK.test(text)
}

/**
 * Does this line start a block that a paragraph cannot swallow?
 *
 * Used in two places that must agree: where a paragraph stops, and where a blockquote's lazy
 * continuation stops. They disagreeing is how a `---` under a paragraph inside a quote becomes
 * either a heading or a rule depending on which caller you ask.
 *
 * Indented code is **not** in here, deliberately: four spaces cannot interrupt a paragraph, they
 * are a continuation of it. That is CommonMark's rule and it is the one people rely on when they
 * hang-indent a long sentence.
 */
function startsBlock(text: string): boolean {
  return (
    FENCE.test(text) ||
    ATX.test(text) ||
    RULE.test(text) ||
    QUOTE.test(text) ||
    BULLET.test(text) ||
    ORDERED.test(text)
  )
}

/* --- the container walk -------------------------------------------------------------------- */

/**
 * Parse a run of lines that have already had their container marker stripped.
 *
 * The order of the tests is the grammar. `RULE` is tried before the list markers because `- - -`
 * matches both and a thematic break wins; the fence is first because nothing inside one is
 * anything but text.
 */
function parseBlocks(lines: readonly Line[], refs: Map<string, LinkDef>): Raw[] {
  const out: Raw[] = []
  let i = 0

  while (i < lines.length) {
    const line = lines[i]
    if (line === undefined) break

    if (isBlank(line.text)) {
      i++
      continue
    }

    const fence = FENCE.exec(line.text)
    if (fence !== null) {
      i = takeFence(lines, i, fence, out)
      continue
    }

    const atx = ATX.exec(line.text)
    if (atx !== null) {
      const hashes = atx[1] ?? ''
      // A closing run of `#`s is decoration, not content: `## Title ##`.
      const text = (atx[2] ?? '').replace(/[ \t]+#+[ \t]*$/, '')
      out.push({ k: 'heading', line: line.no, level: hashes.length, text })
      i++
      continue
    }

    if (RULE.test(line.text)) {
      out.push({ k: 'rule', line: line.no })
      i++
      continue
    }

    if (QUOTE.test(line.text)) {
      i = takeQuote(lines, i, refs, out)
      continue
    }

    if (BULLET.test(line.text) || ORDERED.test(line.text)) {
      i = takeList(lines, i, refs, out)
      continue
    }

    if (INDENTED.test(line.text)) {
      i = takeIndentedCode(lines, i, out)
      continue
    }

    const next = lines[i + 1]
    if (
      line.text.includes('|') &&
      next !== undefined &&
      TABLE_DELIM.test(next.text) &&
      next.text.includes('-')
    ) {
      const taken = takeTable(lines, i, out)
      if (taken !== null) {
        i = taken
        continue
      }
    }

    i = takeParagraph(lines, i, refs, out)
  }

  return out
}

/** A fenced code block, closed by a run of the same character at least as long. */
function takeFence(lines: readonly Line[], start: number, m: RegExpExecArray, out: Raw[]): number {
  const open = lines[start]
  if (open === undefined) return start + 1
  const marker = m[1] ?? '```'
  const ch = marker.charAt(0)
  const info = (m[2] ?? '').trim()
  /*
   * The info string's first word, lowercased, is the language. A backtick fence may not have a
   * backtick in its info string — that is how ``` `code` ``` on one line stays a code span
   * rather than opening a block.
   */
  const lang = ch === '`' && info.includes('`') ? null : (info.split(/\s+/)[0] ?? '').toLowerCase()
  const indent = open.text.length - open.text.trimStart().length

  const body: string[] = []
  let i = start + 1
  for (; i < lines.length; i++) {
    const line = lines[i]
    if (line === undefined) break
    const close = FENCE.exec(line.text)
    if (close !== null) {
      const run = close[1] ?? ''
      if (run.charAt(0) === ch && run.length >= marker.length && (close[2] ?? '').trim() === '') {
        i++
        break
      }
    }
    // The opening fence's indentation is removed from each line, up to that many spaces.
    let text = line.text
    for (let k = 0; k < indent && text.startsWith(' '); k++) text = text.slice(1)
    body.push(text)
  }

  out.push({ k: 'code', line: open.no, lang: lang === '' ? null : lang, text: body.join('\n') })
  return i
}

/** Four-space-indented code: everything until a non-blank line that is not indented. */
function takeIndentedCode(lines: readonly Line[], start: number, out: Raw[]): number {
  const first = lines[start]
  if (first === undefined) return start + 1
  const body: string[] = []
  let i = start
  let pendingBlanks = 0
  for (; i < lines.length; i++) {
    const line = lines[i]
    if (line === undefined) break
    if (isBlank(line.text)) {
      pendingBlanks++
      continue
    }
    if (!INDENTED.test(line.text)) break
    for (let k = 0; k < pendingBlanks; k++) body.push('')
    pendingBlanks = 0
    body.push(line.text.startsWith('\t') ? line.text.slice(1) : line.text.slice(4))
  }
  // Trailing blanks belong to the document, not to the block.
  out.push({ k: 'code', line: first.no, lang: null, text: body.join('\n') })
  return i - pendingBlanks
}

/**
 * A blockquote, including lazy continuation lines.
 *
 * "Lazy" is the rule that lets a wrapped paragraph inside a quote drop the `>` on its second
 * line. It applies only to paragraph text: a line that would start a block of its own ends the
 * quote instead, which is what [`startsBlock`] is for.
 */
function takeQuote(
  lines: readonly Line[],
  start: number,
  refs: Map<string, LinkDef>,
  out: Raw[],
): number {
  const first = lines[start]
  if (first === undefined) return start + 1
  const inner: Line[] = []
  let i = start
  let lastWasText = false

  for (; i < lines.length; i++) {
    const line = lines[i]
    if (line === undefined) break
    if (QUOTE.test(line.text)) {
      // `> ` — one optional space after the marker is part of the marker.
      const stripped = line.text.replace(/^ {0,3}> ?/, '')
      inner.push({ text: stripped, no: line.no })
      lastWasText = !isBlank(stripped)
      continue
    }
    if (lastWasText && !isBlank(line.text) && !startsBlock(line.text)) {
      inner.push({ text: line.text, no: line.no })
      continue
    }
    break
  }

  out.push({ k: 'quote', line: first.no, body: parseBlocks(inner, refs) })
  return i
}

/* --- lists --------------------------------------------------------------------------------- */

interface Marker {
  /** Columns before the marker. */
  indent: number
  ordered: boolean
  /** `-`, `*`, `+` for a bullet; `.` or `)` for an ordered list. Changing it starts a new list. */
  delim: string
  start: number
  /** Columns a continuation line must be indented by to belong to this item. */
  content: number
  /** The item's text on the marker's own line. */
  rest: string
}

function marker(text: string): Marker | null {
  const bullet = BULLET.exec(text)
  if (bullet !== null) {
    const indent = (bullet[1] ?? '').length
    const spaces = (bullet[3] ?? '').length
    return {
      indent,
      ordered: false,
      delim: bullet[2] ?? '-',
      start: 1,
      // More than four spaces after the marker means the content is indented code inside an
      // otherwise empty item, so the item's own content column is one space past the marker.
      content: indent + 1 + (spaces === 0 || spaces > 4 ? 1 : spaces),
      rest: text.slice(bullet[0].length),
    }
  }
  const ordered = ORDERED.exec(text)
  if (ordered !== null) {
    const indent = (ordered[1] ?? '').length
    const digits = ordered[2] ?? '1'
    const spaces = (ordered[4] ?? '').length
    return {
      indent,
      ordered: true,
      delim: ordered[3] ?? '.',
      start: Number.parseInt(digits, 10),
      content: indent + digits.length + 1 + (spaces === 0 || spaces > 4 ? 1 : spaces),
      rest: text.slice(ordered[0].length),
    }
  }
  return null
}

/** Remove up to `n` columns of leading whitespace, counting a tab as four. */
function dedent(text: string, n: number): string {
  let i = 0
  let col = 0
  while (i < text.length && col < n) {
    const ch = text.charAt(i)
    if (ch === ' ') col += 1
    else if (ch === '\t') col += 4
    else break
    i++
  }
  return text.slice(i)
}

/**
 * A whole list: consecutive items sharing a marker kind.
 *
 * The list ends when a line is neither a matching marker nor indented into the current item.
 * Changing the bullet character or the ordered delimiter starts a *new* list — that is
 * CommonMark's rule and it is the only way to put two lists next to each other with no prose
 * between them, which people do deliberately.
 */
function takeList(
  lines: readonly Line[],
  start: number,
  refs: Map<string, LinkDef>,
  out: Raw[],
): number {
  const first = lines[start]
  const head = first === undefined ? null : marker(first.text)
  if (first === undefined || head === null) return start + 1

  const items: RawItem[] = []
  let loose = false
  let i = start

  while (i < lines.length) {
    const line = lines[i]
    if (line === undefined) break
    const m = marker(line.text)
    if (m === null || m.ordered !== head.ordered || m.delim !== head.delim || m.indent > 3) break

    const body: Line[] = [{ text: m.rest, no: line.no }]
    let blanks = 0
    let sawBlankInside = false
    let lastWasText = !isBlank(m.rest)
    i++

    for (; i < lines.length; i++) {
      const cont = lines[i]
      if (cont === undefined) break
      if (isBlank(cont.text)) {
        blanks++
        continue
      }
      const contIndent = cont.text.length - cont.text.trimStart().length
      if (contIndent >= m.content) {
        // Blank lines only count once we know the item continues past them.
        for (let k = 0; k < blanks; k++) body.push({ text: '', no: cont.no })
        if (blanks > 0) sawBlankInside = true
        blanks = 0
        body.push({ text: dedent(cont.text, m.content), no: cont.no })
        lastWasText = true
        continue
      }
      if (blanks === 0 && lastWasText && !startsBlock(cont.text)) {
        // Lazy continuation of the item's paragraph, exactly as in a blockquote.
        body.push({ text: cont.text, no: cont.no })
        continue
      }
      break
    }

    const parsed = parseBlocks(body, refs)
    /*
     * `- [ ] thing` — GFM's task item. Recognised after parsing rather than before, so that the
     * marker has to be at the start of the item's first *paragraph*: `- ![img]` is not a task,
     * and neither is a `[ ]` two lines down.
     */
    let checked: boolean | null = null
    const lead = parsed[0]
    if (lead !== undefined && lead.k === 'para') {
      const task = /^\[([ xX])\](?:[ \t]+|$)/.exec(lead.text)
      if (task !== null) {
        checked = (task[1] ?? ' ') !== ' '
        lead.text = lead.text.slice(task[0].length)
      }
    }

    items.push({ line: line.no, checked, body: parsed })
    if (sawBlankInside) loose = true
    // A blank line between two items makes the whole list loose; one after the last item is
    // simply the end of the list and means nothing.
    if (blanks > 0) {
      const after = lines[i]
      const following = after === undefined ? null : marker(after.text)
      if (
        following !== null &&
        following.ordered === head.ordered &&
        following.delim === head.delim &&
        following.indent <= 3
      ) {
        loose = true
      }
    }
  }

  out.push({
    k: 'list',
    line: first.no,
    ordered: head.ordered,
    start: head.start,
    tight: !loose,
    items,
  })
  return i
}

/* --- tables -------------------------------------------------------------------------------- */

/** Split a row on unescaped pipes, dropping the outer ones. */
function cells(text: string): string[] {
  const parts: string[] = []
  let current = ''
  for (let i = 0; i < text.length; i++) {
    const ch = text.charAt(i)
    if (ch === '\\') {
      // `\|` is a literal pipe inside a cell; the backslash is consumed here so `inline.ts`
      // does not see an escape it would resolve a second time.
      const next = text.charAt(i + 1)
      if (next === '|') {
        current += '|'
        i++
        continue
      }
      current += ch
      continue
    }
    if (ch === '|') {
      parts.push(current)
      current = ''
      continue
    }
    current += ch
  }
  parts.push(current)
  if (parts.length > 0 && (parts[0] ?? '').trim() === '') parts.shift()
  if (parts.length > 0 && (parts[parts.length - 1] ?? '').trim() === '') parts.pop()
  return parts.map((cell) => cell.trim())
}

function alignments(text: string): Align[] {
  return cells(text).map((cell) => {
    const left = cell.startsWith(':')
    const right = cell.endsWith(':')
    if (left && right) return 'center'
    if (right) return 'right'
    if (left) return 'left'
    return null
  })
}

/**
 * A GFM table, or `null` when the delimiter row does not describe the header.
 *
 * The column counts must agree. Without that check a paragraph containing one pipe followed by a
 * line of dashes — which is a setext heading under a sentence with a pipe in it — is silently
 * eaten as a one-column table.
 */
function takeTable(lines: readonly Line[], start: number, out: Raw[]): number | null {
  const header = lines[start]
  const delim = lines[start + 1]
  if (header === undefined || delim === undefined) return null

  const head = cells(header.text)
  const align = alignments(delim.text)
  if (head.length === 0 || head.length !== align.length) return null

  const rows: { line: number; cells: string[] }[] = []
  let i = start + 2
  for (; i < lines.length; i++) {
    const line = lines[i]
    if (line === undefined) break
    if (isBlank(line.text) || !line.text.includes('|') || startsBlock(line.text)) break
    const row = cells(line.text)
    // Short rows are padded and long ones truncated, which is what every GFM renderer does and
    // what stops one malformed row from shifting every column below it.
    while (row.length < head.length) row.push('')
    rows.push({ line: line.no, cells: row.slice(0, head.length) })
  }

  out.push({ k: 'table', line: header.no, head, align, rows })
  return i
}

/* --- paragraphs and link definitions -------------------------------------------------------- */

/**
 * A paragraph, its setext heading if it has one, and any link definitions off its front.
 *
 * Definitions are stripped here rather than in a pre-pass because only here is it known that
 * these lines are *not* inside a fence — a `[x]: y` in a code block is code. Stripping them can
 * consume the whole paragraph, in which case no block is emitted at all, which is why a file of
 * nothing but definitions renders as nothing rather than as a wall of prose.
 */
function takeParagraph(
  lines: readonly Line[],
  start: number,
  refs: Map<string, LinkDef>,
  out: Raw[],
): number {
  const collected: Line[] = []
  let i = start
  let level = 0

  for (; i < lines.length; i++) {
    const line = lines[i]
    if (line === undefined) break
    if (isBlank(line.text)) break
    if (i > start) {
      const setext = SETEXT.exec(line.text)
      if (setext !== null) {
        level = (setext[1] ?? '=').startsWith('=') ? 1 : 2
        i++
        break
      }
      if (startsBlock(line.text)) break
      const next = lines[i + 1]
      if (
        line.text.includes('|') &&
        next !== undefined &&
        TABLE_DELIM.test(next.text) &&
        next.text.includes('-')
      ) {
        break
      }
    }
    collected.push(line)
  }

  // Link reference definitions, off the front, one line each.
  let from = 0
  while (from < collected.length && level === 0) {
    const line = collected[from]
    if (line === undefined) break
    const def = LINK_DEF.exec(line.text)
    if (def === null) break
    const label = normaliseLabel(def[1] ?? '')
    if (label === '') break
    const title = def[3] ?? def[4] ?? def[5] ?? null
    // First definition wins, which is CommonMark's rule and the only one under which a document
    // that defines a label twice renders the same on every run.
    if (!refs.has(label)) refs.set(label, { href: def[2] ?? '', title })
    from++
  }

  const body = collected.slice(from)
  const first = body[0]
  if (first !== undefined) {
    const text = body.map((line) => line.text.trim()).join('\n')
    if (level === 0) out.push({ k: 'para', line: first.no, text })
    else out.push({ k: 'heading', line: first.no, level, text })
  }
  return Math.max(i, start + 1)
}

/* --- the inline pass ------------------------------------------------------------------------ */

function finishBlocks(
  raw: readonly Raw[],
  refs: ReadonlyMap<string, LinkDef>,
  opts: MarkdownOptions,
): Block[] {
  const out: Block[] = []
  for (const block of raw) {
    switch (block.k) {
      case 'heading': {
        // The level came from counting `#`s or from a setext underline; both are already bounded
        // to 1–6, and the cast is what tells the type system so.
        const level = Math.min(6, Math.max(1, block.level)) as 1 | 2 | 3 | 4 | 5 | 6
        out.push({
          kind: 'heading',
          line: block.line,
          level,
          body: parseInline(block.text, refs, opts),
        })
        break
      }
      case 'para':
        out.push({
          kind: 'paragraph',
          line: block.line,
          body: parseInline(block.text, refs, opts),
        })
        break
      case 'code':
        out.push({ kind: 'code', line: block.line, lang: block.lang, text: block.text })
        break
      case 'rule':
        out.push({ kind: 'rule', line: block.line })
        break
      case 'quote':
        out.push({ kind: 'quote', line: block.line, body: finishBlocks(block.body, refs, opts) })
        break
      case 'list': {
        const items: ListItem[] = block.items.map((item) => ({
          line: item.line,
          checked: item.checked,
          body: finishBlocks(item.body, refs, opts),
        }))
        out.push({
          kind: 'list',
          line: block.line,
          ordered: block.ordered,
          start: block.start,
          tight: block.tight,
          items,
        })
        break
      }
      case 'table': {
        const head: Inline[][] = block.head.map((cell) => parseInline(cell, refs, opts))
        const rows: TableRow[] = block.rows.map((row) => ({
          line: row.line,
          cells: row.cells.map((cell) => parseInline(cell, refs, opts)),
        }))
        out.push({ kind: 'table', line: block.line, head, align: block.align, rows })
        break
      }
    }
  }
  return out
}

/**
 * Parse a whole markdown document.
 *
 * `\r\n` and bare `\r` are normalised here rather than by the caller, because the caller is the
 * live CodeMirror buffer and `editor/lineEndings.ts` deliberately keeps a document's original
 * breaks: the preview must not be the reason a file's line endings are a question.
 *
 * `opts` defaults to CommonMark in every respect — see `MarkdownOptions` in `types.ts` for the
 * one thing a caller can change and why only a caller rendering a *message* should change it.
 */
export function parseMarkdown(source: string, opts: MarkdownOptions = {}): MarkdownDoc {
  const text = source.replace(/\r\n?/g, '\n')
  const lines: Line[] = text.split('\n').map((value, index) => ({ text: value, no: index + 1 }))
  /*
   * A trailing newline terminates the last line; it does not begin an empty one. Keeping it costs
   * a blank line at the end of every unclosed fence and an extra `docLines` for scroll sync to
   * interpolate towards, and every file in this repository ends with one.
   */
  if (lines.length > 1 && (lines[lines.length - 1]?.text ?? '') === '') lines.pop()
  const refs = new Map<string, LinkDef>()
  const blocks = finishBlocks(parseBlocks(lines, refs), refs, opts)
  return { blocks, lines: blocks.map((block) => block.line) }
}
