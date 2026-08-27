/**
 * Inline markdown: emphasis, code spans, links, images, breaks. (M20)
 *
 * The counterpart to `blocks.ts`, which decides what a *line* is. This decides what the
 * characters inside one paragraph, heading or table cell are, and it is the half where every
 * performance trap in markdown lives.
 *
 * # The two bounds, both of which are load-bearing
 *
 * `languages/markdown.ts` already paid for this lesson once and wrote the measurement down:
 * an unbounded `\[[^\]]*\]\(` "scans to the end of the line for every `[` that does not open a
 * link and then backtracks over the whole scan one character at a time — so a line of brackets
 * is quadratic. Measured at 3.8 s for a 160,000-character line of `[`, against 0.13 s for the
 * same line with this bound." [`LINK_SCAN_LIMIT`] is that same 512, for that same reason, and a
 * link whose text or destination runs past it is drawn as prose.
 *
 * The second bound is structural rather than numeric. Emphasis resolution is CommonMark's
 * delimiter-stack algorithm, and it is run over a **doubly linked list** rather than an array.
 * Matching a pair removes the nodes between two delimiters, and `Array.prototype.splice` shifts
 * every element after the cut: on `*a*` repeated *n* times — an ordinary emphasis-heavy
 * paragraph, not a pathological input — that is O(n²). The list makes it O(1) per match.
 * `openers_bottom` is the other half of the same guarantee: without it, a run of unmatchable
 * closers re-scans the whole opener stack for each one.
 *
 * # There is no `html` node, at any level
 *
 * `<b>hi</b>` in a document produces the eight characters. `types.ts`'s header has the argument;
 * the consequence here is that `<` is an ordinary character with exactly one special case —
 * `<https://…>`, which is an autolink and produces a `link` whose href is the text.
 *
 * Import-free apart from its own types, so `ui/scripts/check-markdown.mjs` can compile and run
 * it with no bundler.
 */
import type { Inline, LinkDef, MarkdownOptions } from './types'

/**
 * How far past a `[` or a `(` the link matcher will look for its closer.
 *
 * The same 512 as `languages/markdown.ts::LINK_SCAN_LIMIT`, and deliberately the same number
 * rather than a second opinion: the two are answering one question — how much of a line may a
 * bracket claim — and a preview that renders a link the buffer refuses to colour, or the
 * reverse, is a difference a reader would report as a bug in whichever half they trusted less.
 */
export const LINK_SCAN_LIMIT = 512

/**
 * How deep `[text](…)` may nest before the inner text is left as prose.
 *
 * Link text is parsed by recursion, so depth multiplies the work: `[[[[…]]]]` with *n* brackets
 * is n/2 levels over a shrinking string. The cap makes that product linear, and eight is far
 * past anything a person writes — the deepest construct in real documents is a link inside a
 * table cell inside a list, which is one level here.
 */
const MAX_NESTING = 8

/* --- the intermediate list ----------------------------------------------------------------- */

/**
 * One node of the working list.
 *
 * A single mutable shape rather than a discriminated union of node classes, because emphasis
 * resolution *rewrites* nodes in place — a `delim` that ends up matching becomes the `wrap` that
 * replaces it, and a `delim` that never matches becomes `text`. A union would mean allocating a
 * replacement and re-linking on every one of those, which is the allocation this design is
 * trying not to do per character.
 */
interface Node {
  t: 'text' | 'code' | 'break' | 'link' | 'image' | 'delim' | 'wrap'
  prev: Node | null
  next: Node | null
  /** `text`, `code`: the characters. `delim`: the run, so an unmatched one is already its text. */
  s: string
  /** `link`, `image`. */
  href: string
  title: string | null
  /** `image`: the flattened alt text. */
  alt: string
  /** `link`, `wrap`: the first child, or `null` for an empty body. */
  head: Node | null
  /** `wrap`: which element this became. */
  wrap: 'strong' | 'em' | 'del'
  /** `delim`: the run's character and how many are left unconsumed. */
  ch: string
  n: number
  canOpen: boolean
  canClose: boolean
}

function node(t: Node['t']): Node {
  return {
    t,
    prev: null,
    next: null,
    s: '',
    href: '',
    title: null,
    alt: '',
    head: null,
    wrap: 'em',
    ch: '',
    n: 0,
    canOpen: false,
    canClose: false,
  }
}

/** A list under construction: `head` for the walk, `tail` for the append. */
interface List {
  head: Node | null
  tail: Node | null
}

function push(list: List, n: Node): void {
  n.prev = list.tail
  n.next = null
  if (list.tail === null) list.head = n
  else list.tail.next = n
  list.tail = n
}

/** Append `text`, merging into the previous node when it is also text. */
function pushText(list: List, text: string): void {
  if (text === '') return
  const last = list.tail
  if (last !== null && last.t === 'text') {
    last.s += text
    return
  }
  const n = node('text')
  n.s = text
  push(list, n)
}

/** Unlink everything strictly between `a` and `b`. Returns the removed run's head. */
function extractBetween(a: Node, b: Node): Node | null {
  const first = a.next
  if (first === null || first === b) {
    a.next = b
    b.prev = a
    return null
  }
  const last = b.prev
  a.next = b
  b.prev = a
  first.prev = null
  if (last !== null) last.next = null
  return first
}

/* --- character classes --------------------------------------------------------------------- */

/**
 * Unicode punctuation, for the flanking rules.
 *
 * `\p{P}` and `\p{S}` rather than an ASCII list: `«mot»` and `„Wort“` are the cases an ASCII
 * test gets wrong, and they are exactly the ones a non-English document is full of. The `u`
 * flag is required for `\p{…}` and is available everywhere this runs (es2023).
 */
const PUNCT = /[\p{P}\p{S}]/u
const SPACE = /\s/

function isPunct(ch: string): boolean {
  return ch !== '' && PUNCT.test(ch)
}

function isSpace(ch: string): boolean {
  return ch === '' || SPACE.test(ch)
}

/* --- scanning helpers ---------------------------------------------------------------------- */

/**
 * The index of the `]` closing the `[` at `open`, or `-1`.
 *
 * Bracket-aware, escape-aware and code-span-aware, because `[a `]` b](x)` is a link whose text
 * contains a code span holding a bracket. Bounded by [`LINK_SCAN_LIMIT`]; a `[` with no `]`
 * within that many characters is not a link, which is the refusal the bound exists to make
 * cheap.
 */
function closingBracket(src: string, open: number): number {
  let depth = 0
  const limit = Math.min(src.length, open + LINK_SCAN_LIMIT)
  for (let i = open; i < limit; i++) {
    const ch = src.charAt(i)
    if (ch === '\\') {
      i++
      continue
    }
    if (ch === '`') {
      const run = runLength(src, i, '`')
      const end = findCodeEnd(src, i + run, run, limit)
      if (end !== -1) {
        i = end + run - 1
        continue
      }
      i += run - 1
      continue
    }
    if (ch === '[') depth++
    else if (ch === ']') {
      depth--
      if (depth === 0) return i
    }
  }
  return -1
}

/** How many copies of `ch` start at `i`. */
function runLength(src: string, i: number, ch: string): number {
  let n = 0
  while (src.charAt(i + n) === ch) n++
  return n
}

/** The start of the next run of exactly `want` backticks at or after `from`, or `-1`. */
function findCodeEnd(src: string, from: number, want: number, limit: number): number {
  for (let i = from; i < limit; i++) {
    if (src.charAt(i) !== '`') continue
    const run = runLength(src, i, '`')
    if (run === want) return i
    i += run - 1
  }
  return -1
}

/** What `(dest "title")` starting at `open` holds, or `null` if it is not one. */
function inlineDestination(
  src: string,
  open: number,
): { href: string; title: string | null; end: number } | null {
  const limit = Math.min(src.length, open + LINK_SCAN_LIMIT)
  let i = open + 1
  while (i < limit && isSpace(src.charAt(i))) i++

  let href = ''
  if (src.charAt(i) === '<') {
    // An angle destination may hold spaces and parentheses; it may not hold a newline.
    i++
    while (i < limit && src.charAt(i) !== '>' && src.charAt(i) !== '\n') {
      if (src.charAt(i) === '\\') i++
      href += src.charAt(i)
      i++
    }
    if (src.charAt(i) !== '>') return null
    i++
  } else {
    let depth = 0
    while (i < limit) {
      const ch = src.charAt(i)
      if (ch === '\\') {
        i++
        href += src.charAt(i)
        i++
        continue
      }
      if (isSpace(ch)) break
      if (ch === '(') depth++
      if (ch === ')') {
        if (depth === 0) break
        depth--
      }
      href += ch
      i++
    }
  }

  while (i < limit && isSpace(src.charAt(i))) i++

  let title: string | null = null
  const quote = src.charAt(i)
  if (quote === '"' || quote === "'" || quote === '(') {
    const close = quote === '(' ? ')' : quote
    i++
    let text = ''
    while (i < limit && src.charAt(i) !== close) {
      if (src.charAt(i) === '\\') i++
      text += src.charAt(i)
      i++
    }
    if (src.charAt(i) !== close) return null
    i++
    title = text
    while (i < limit && isSpace(src.charAt(i))) i++
  }

  if (src.charAt(i) !== ')') return null
  return { href, title, end: i }
}

/** A reference label, normalised the way CommonMark matches them: folded and space-collapsed. */
export function normaliseLabel(label: string): string {
  return label.trim().replace(/\s+/g, ' ').toLowerCase()
}

/* --- the tokenizer ------------------------------------------------------------------------- */

/**
 * Characters that end a plain text run.
 *
 * A set rather than a regex `lastIndex` walk so the scan is one pass with no backtracking. `h`
 * is in here only because of bare `http://` autolinking — every other member is structural.
 */
const SPECIAL = new Set(['\\', '`', '<', '[', '!', '*', '_', '~', '\n', 'h', 'w'])

function tokenize(
  src: string,
  refs: ReadonlyMap<string, LinkDef>,
  depth: number,
  opts: MarkdownOptions,
): List {
  const list: List = { head: null, tail: null }
  const n = src.length
  let i = 0

  while (i < n) {
    const ch = src.charAt(i)

    if (!SPECIAL.has(ch)) {
      // The common case: run to the next character that could mean something.
      let j = i + 1
      while (j < n && !SPECIAL.has(src.charAt(j))) j++
      pushText(list, src.slice(i, j))
      i = j
      continue
    }

    if (ch === '\\') {
      const next = src.charAt(i + 1)
      if (next === '\n') {
        // A trailing backslash is a hard break — the spelling that survives an editor that
        // strips trailing whitespace, which is why it exists and why it is worth supporting.
        push(list, node('break'))
        i += 2
        continue
      }
      if (isPunct(next)) {
        pushText(list, next)
        i += 2
        continue
      }
      pushText(list, '\\')
      i++
      continue
    }

    if (ch === '`') {
      const run = runLength(src, i, '`')
      const end = findCodeEnd(src, i + run, run, n)
      if (end === -1) {
        pushText(list, src.slice(i, i + run))
        i += run
        continue
      }
      const code = node('code')
      /*
       * One leading and one trailing space are stripped when *both* are present, which is the
       * rule that lets `` ` `` `` `` be a code span holding a backtick. Interior runs of
       * whitespace, including newlines, collapse to single spaces because a code *span* is one
       * line however the source wrapped it.
       */
      let text = src.slice(i + run, end).replace(/\n/g, ' ')
      if (text.length > 1 && text.startsWith(' ') && text.endsWith(' ') && text.trim() !== '') {
        text = text.slice(1, -1)
      }
      code.s = text
      push(list, code)
      i = end + run
      continue
    }

    if (ch === '<') {
      const auto = /^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^<>\s]*|[^\s<>@]+@[^\s<>@]+\.[^\s<>@]+)>/.exec(
        src.slice(i, i + LINK_SCAN_LIMIT),
      )
      if (auto !== null) {
        const target = auto[1] ?? ''
        const link = node('link')
        link.href = target.includes('@') && !target.includes(':') ? `mailto:${target}` : target
        const label = node('text')
        label.s = target
        link.head = label
        push(list, link)
        i += auto[0].length
        continue
      }
      // Everything else that looks like a tag is text. See the header.
      pushText(list, '<')
      i++
      continue
    }

    if (ch === 'h' || ch === 'w') {
      /*
       * A bare URL. `w` is here for `www.`, which GitHub linkifies and which appears in enough
       * READMEs to be worth the branch.
       *
       * Gated on a word boundary and matched with `startsWith` at an index rather than a regex
       * over a slice. Prose is full of `h` and `w`, so this branch runs on a large fraction of
       * all characters; slicing 512 of them each time would make an ordinary paragraph cost
       * O(n · 512) for nothing. `startsWith(…, i)` allocates nothing.
       *
       * The trailing-punctuation trim is what stops `see https://example.com.` from producing a
       * link whose href ends in a full stop, and the unbalanced-paren trim is the Wikipedia
       * case: `https://en.wikipedia.org/wiki/Ruby_(gem)` keeps its parenthesis, while
       * `(see https://example.com)` does not.
       */
      const before = i === 0 ? '' : src.charAt(i - 1)
      const boundary = i === 0 || isSpace(before) || '([<'.includes(before)
      const scheme =
        src.startsWith('https://', i) || src.startsWith('http://', i) || src.startsWith('www.', i)
      if (boundary && scheme) {
        let j = i
        while (j < n && !isSpace(src.charAt(j)) && src.charAt(j) !== '<' && src.charAt(j) !== '>') {
          j++
        }
        let target = src.slice(i, j)
        for (;;) {
          const last = target.charAt(target.length - 1)
          if (last === ')' && count(target, '(') < count(target, ')')) target = target.slice(0, -1)
          else if ('.,;:!?'.includes(last)) target = target.slice(0, -1)
          else break
        }
        // `https://` on its own is not a link; there has to be something after the scheme.
        const rest = target.replace(/^(?:https?:\/\/|www\.)/, '')
        if (rest !== '') {
          const link = node('link')
          link.href = target.startsWith('www.') ? `https://${target}` : target
          link.head = textOnly(target)
          push(list, link)
          i += target.length
          continue
        }
      }
      pushText(list, ch)
      i++
      continue
    }

    if (ch === '[' || (ch === '!' && src.charAt(i + 1) === '[')) {
      const image = ch === '!'
      const open = image ? i + 1 : i
      const close = closingBracket(src, open)
      if (close !== -1) {
        const inner = src.slice(open + 1, close)
        const target = resolveTarget(src, close, inner, refs)
        if (target !== null) {
          if (image) {
            const img = node('image')
            img.href = target.def.href
            img.title = target.def.title
            img.alt = flatten(inner)
            push(list, img)
          } else {
            const link = node('link')
            link.href = target.def.href
            link.title = target.def.title
            link.head =
              depth >= MAX_NESTING
                ? textOnly(inner)
                : parseNodes(inner, refs, depth + 1, opts).head
            push(list, link)
          }
          i = target.end
          continue
        }
      }
      pushText(list, image ? '![' : '[')
      i += image ? 2 : 1
      continue
    }

    if (ch === '*' || ch === '_' || ch === '~') {
      const run = runLength(src, i, ch)
      if (ch === '~' && run !== 2) {
        // GFM strikethrough is exactly two. A lone `~` is a tilde, and `~~~` is a fence the
        // block parser has already taken, so anything reaching here is text.
        pushText(list, src.slice(i, i + run))
        i += run
        continue
      }
      const before = i === 0 ? '' : src.charAt(i - 1)
      const after = src.charAt(i + run)
      const leftFlanking =
        !isSpace(after) && (!isPunct(after) || isSpace(before) || isPunct(before))
      const rightFlanking =
        !isSpace(before) && (!isPunct(before) || isSpace(after) || isPunct(after))
      const d = node('delim')
      d.ch = ch
      d.n = run
      d.s = src.slice(i, i + run)
      /*
       * `_` may not open or close *inside* a word, and `*` may. That one asymmetry is the whole
       * of why `snake_case_names` survives a markdown renderer and `a*b*c` does not.
       */
      d.canOpen = ch === '_' ? leftFlanking && (!rightFlanking || isPunct(before)) : leftFlanking
      d.canClose = ch === '_' ? rightFlanking && (!leftFlanking || isPunct(after)) : rightFlanking
      push(list, d)
      i += run
      continue
    }

    if (ch === '\n') {
      /*
       * Two or more spaces before a newline is a hard break; otherwise the break is soft and
       * renders as a space, because a paragraph is one flow however the source was wrapped —
       * unless the caller asked for `softBreak: 'break'`, which is a message rather than a
       * document and means the line it wrote (`types.ts`'s `MarkdownOptions` carries the
       * argument). Either way the trailing whitespace is trimmed off the text node first: it is
       * the *spelling* of the hard break, never content.
       */
      const last = list.tail
      let hard = false
      if (last !== null && last.t === 'text') {
        const trimmed = last.s.replace(/[ \t]+$/, '')
        hard = last.s.length - trimmed.length >= 2
        last.s = trimmed
        if (last.s === '') {
          // An all-whitespace text node would render as a stray space at the start of a line.
          list.tail = last.prev
          if (list.tail === null) list.head = null
          else list.tail.next = null
        }
      }
      if (hard || opts.softBreak === 'break') push(list, node('break'))
      else pushText(list, ' ')
      i++
      // Leading whitespace on the continuation line is not content.
      while (i < n && (src.charAt(i) === ' ' || src.charAt(i) === '\t')) i++
      continue
    }

    /*
     * A character that is in `SPECIAL` but did not start anything — a `!` with no `[` after it,
     * a `~` that was not a pair. It is text, and the loop must advance: falling through to the
     * next iteration without consuming it is the shape that hangs the renderer on one `!`.
     */
    pushText(list, ch)
    i++
  }

  return list
}

function count(s: string, ch: string): number {
  let n = 0
  for (let i = 0; i < s.length; i++) if (s.charAt(i) === ch) n++
  return n
}

/** A single text node holding `s`, for a body that may not be parsed any further. */
function textOnly(s: string): Node {
  const n = node('text')
  n.s = s
  return n
}

/**
 * What the `[…]` ending at `close` points at, and where the whole construct ends.
 *
 * Three spellings, in the order CommonMark tries them: inline `(…)`, full reference `[id]`, and
 * collapsed/shortcut, where the text is its own label. `null` means "not a link" and the caller
 * emits a literal bracket.
 */
function resolveTarget(
  src: string,
  close: number,
  inner: string,
  refs: ReadonlyMap<string, LinkDef>,
): { def: LinkDef; end: number } | null {
  if (src.charAt(close + 1) === '(') {
    const dest = inlineDestination(src, close + 1)
    if (dest !== null) return { def: { href: dest.href, title: dest.title }, end: dest.end + 1 }
  }
  if (src.charAt(close + 1) === '[') {
    const end = closingBracket(src, close + 1)
    if (end !== -1) {
      const label = src.slice(close + 2, end)
      const key = normaliseLabel(label === '' ? inner : label)
      const def = refs.get(key)
      if (def !== undefined) return { def, end: end + 1 }
      return null
    }
  }
  const def = refs.get(normaliseLabel(inner))
  if (def !== undefined) return { def, end: close + 1 }
  return null
}

/** Link text, as characters, for an `alt=` attribute. */
function flatten(src: string): string {
  return src.replace(/\\([\p{P}\p{S}])/gu, '$1').replace(/[*_~`]/g, '')
}

/* --- emphasis ------------------------------------------------------------------------------ */

/**
 * CommonMark's delimiter-stack pass, over the linked list.
 *
 * The shape is the reference algorithm's: walk forward to each potential closer, walk back to
 * the nearest compatible opener, consume one or two delimiters from each end, and wrap what was
 * between them. Two details are not decoration:
 *
 * * **`openersBottom`** records, per (character, closer length mod 3, can-this-also-open), how
 *   far back it is worth looking. Without it a paragraph of unmatchable closers re-walks the
 *   whole stack for each one, which is the quadratic `check-markdown.mjs` measures.
 * * **The rule of three** — a pair whose lengths sum to a multiple of three may not match when
 *   either side can both open and close — is what makes `*foo**bar**baz*` nest the way a reader
 *   expects rather than pairing the outer star with the first inner one.
 */
function processEmphasis(list: List): void {
  const openersBottom = new Map<string, Node | null>()

  let closer = list.head
  while (closer !== null) {
    if (closer.t !== 'delim' || !closer.canClose) {
      closer = closer.next
      continue
    }

    const key = `${closer.ch}${closer.n % 3}${closer.canOpen ? '1' : '0'}`
    const bottom = openersBottom.get(key) ?? null

    let opener: Node | null = closer.prev
    let found = false
    while (opener !== null && opener !== bottom) {
      if (opener.t === 'delim' && opener.ch === closer.ch && opener.canOpen) {
        const oddMatch =
          (closer.canOpen || opener.canClose) &&
          closer.n % 3 !== 0 &&
          (opener.n + closer.n) % 3 === 0
        if (!oddMatch) {
          found = true
          break
        }
      }
      opener = opener.prev
    }

    if (!found || opener === null) {
      /*
       * Nothing back there can match this closer, and nothing ever will — a later closer with
       * the same key would walk the same dead stretch. Record how far, and if this delimiter
       * cannot open either, it is simply text.
       */
      openersBottom.set(key, closer.prev)
      const next: Node | null = closer.next
      if (!closer.canOpen) {
        closer.t = 'text'
      }
      closer = next
      continue
    }

    const strong = opener.n >= 2 && closer.n >= 2
    const take = closer.ch === '~' ? 2 : strong ? 2 : 1
    const wrap = node('wrap')
    wrap.wrap = closer.ch === '~' ? 'del' : strong ? 'strong' : 'em'
    wrap.head = extractBetween(opener, closer)

    opener.n -= take
    closer.n -= take
    opener.s = opener.s.slice(take)
    closer.s = closer.s.slice(take)

    // The wrapper goes where the consumed delimiters met.
    wrap.prev = opener
    wrap.next = closer
    opener.next = wrap
    closer.prev = wrap

    if (opener.n === 0) unlink(list, opener)
    if (closer.n === 0) {
      const after: Node | null = closer.next
      unlink(list, closer)
      closer = after
    }
    // A closer with delimiters left over stays where it is and is tried again.
  }

  // Anything still a delimiter never matched; its characters are its text.
  for (let n = list.head; n !== null; n = n.next) if (n.t === 'delim') n.t = 'text'
}

function unlink(list: List, n: Node): void {
  if (n.prev === null) list.head = n.next
  else n.prev.next = n.next
  if (n.next === null) list.tail = n.prev
  else n.next.prev = n.prev
}

/* --- output -------------------------------------------------------------------------------- */

function toInline(head: Node | null): Inline[] {
  const out: Inline[] = []
  for (let n = head; n !== null; n = n.next) {
    switch (n.t) {
      case 'text':
      case 'delim': {
        if (n.s === '') break
        /*
         * Merged with whatever came before it, and that is not tidiness. Every delimiter that
         * failed to match arrives here as its own node — `snake_case_name` is five of them — and
         * one `<span>` per fragment is a DOM node per underscore in a document full of
         * identifiers, plus five text nodes where a reader's selection expects one word.
         */
        const previous = out[out.length - 1]
        if (previous !== undefined && previous.kind === 'text') {
          out[out.length - 1] = { kind: 'text', text: previous.text + n.s }
        } else {
          out.push({ kind: 'text', text: n.s })
        }
        break
      }
      case 'code':
        out.push({ kind: 'code', text: n.s })
        break
      case 'break':
        out.push({ kind: 'break' })
        break
      case 'image':
        out.push({ kind: 'image', src: n.href, title: n.title, alt: n.alt })
        break
      case 'link':
        out.push({ kind: 'link', href: n.href, title: n.title, body: toInline(n.head) })
        break
      case 'wrap': {
        const body = toInline(n.head)
        if (n.wrap === 'strong') out.push({ kind: 'strong', body })
        else if (n.wrap === 'em') out.push({ kind: 'em', body })
        else out.push({ kind: 'del', body })
        break
      }
    }
  }
  return out
}

/** Tokenize and resolve emphasis. The whole pipeline, so a link's text gets it too. */
function parseNodes(
  src: string,
  refs: ReadonlyMap<string, LinkDef>,
  depth: number,
  opts: MarkdownOptions,
): List {
  const list = tokenize(src, refs, depth, opts)
  processEmphasis(list)
  return list
}

/**
 * Parse one run of inline markdown.
 *
 * `refs` is the document's link reference definitions, which is why this cannot run until
 * `blocks.ts` has read the whole file: `[cide]` on line 3 may be defined on line 900.
 */
export function parseInline(
  src: string,
  refs: ReadonlyMap<string, LinkDef>,
  opts: MarkdownOptions = {},
): Inline[] {
  return toInline(parseNodes(src, refs, 0, opts).head)
}
