/**
 * The formatting toolbar's pure half: which tools exist, and what each does to a draft.
 *
 * # Import-free, like `model.ts` and `mentionModel.ts`, and for the same stated reason
 *
 * `ui/scripts/check-agents.mjs` compiles this file **standalone** with a bare `tsc` — no
 * tsconfig, no path aliases — and drives [`applyTool`] under node. "What does pressing Bold do
 * to the three characters you selected" is exactly the kind of question that is otherwise
 * decided inside a DOM event handler nothing in this repository can run. The icon names in
 * [`MARKDOWN_TOOLS`] are therefore plain `string`s, narrowed by `asIcon` at the render site —
 * the same trade the two glyph tables one file over make, and `check-ui-icons.mjs` reads this
 * file's literals so a stale name still fails the build.
 *
 * # Why the tools write markdown into the draft rather than styling anything
 *
 * The card renders a task's body and its comments as markdown now (`TaskMarkdown.tsx`), so the
 * honest affordance is the one GitHub's comment box offers: buttons that spell the syntax into
 * the text, visibly, where the user can read back exactly what will be committed. A WYSIWYG
 * layer would put a second representation between the user and a file the whole team commits.
 *
 * # Every transform is a toggle where a toggle is expressible
 *
 * Pressing Bold on `**word**` takes the stars off. Without that, the button is a one-way
 * ratchet and the way back is hand-deleting syntax the button put in — which teaches the user
 * the toolbar is for writing only, when the cheap half of its value is repair.
 */

/** The tools, in the order the toolbar draws them. */
export type MarkdownTool =
  | 'bold'
  | 'italic'
  | 'code'
  | 'link'
  | 'heading'
  | 'quote'
  | 'bullet'
  | 'ordered'

export interface ToolSpec {
  tool: MarkdownTool
  /** A vendored icon name. `string` because this module is import-free; see the header. */
  icon: string
  /** The control's `title`/`aria-label`. Names the act, not the syntax. */
  label: string
}

/**
 * Eight, and deliberately not more. Strikethrough, tables and task lists all parse and render,
 * but a toolbar is a reading cost on every open of the card and each button must earn its row;
 * these are the eight GitHub's composer settled on and the ones a comment actually uses.
 */
export const MARKDOWN_TOOLS: readonly ToolSpec[] = [
  { tool: 'bold', icon: 'bold', label: 'Bold' },
  { tool: 'italic', icon: 'italic', label: 'Italic' },
  { tool: 'code', icon: 'code', label: 'Code' },
  { tool: 'link', icon: 'link', label: 'Link' },
  { tool: 'heading', icon: 'heading', label: 'Heading' },
  { tool: 'quote', icon: 'text-quote', label: 'Quote' },
  { tool: 'bullet', icon: 'list', label: 'Bullet list' },
  { tool: 'ordered', icon: 'list-ordered', label: 'Numbered list' },
]

/** Is this a tool this build knows? Takes `string` for `isTaskStatus`'s reason one file over. */
export function isMarkdownTool(value: string): value is MarkdownTool {
  return MARKDOWN_TOOLS.some((spec) => spec.tool === value)
}

/**
 * A draft after a tool ran: the new text, and where the selection lands in it.
 *
 * The selection is part of the result on purpose. A toggle is only a toggle if pressing the
 * button twice returns the original text, and that requires the first press to leave the
 * selection over what it transformed rather than collapsing it to a caret.
 */
export interface ToolEdit {
  text: string
  selStart: number
  selEnd: number
}

/* The inline markers. `*` for italic rather than `_`: `_` inside a word is literal in GFM
   (`snake_case` must survive), so `*` is the spelling that means emphasis everywhere. */
const INLINE_MARKER: Record<string, string> = {
  bold: '**',
  italic: '*',
  code: '`',
}

/* What an empty-selection press leaves selected, so the user types over it rather than hunting
   for the caret between two markers they cannot see the boundary of. */
const INLINE_PLACEHOLDER: Record<string, string> = {
  bold: 'bold',
  italic: 'italic',
  code: 'code',
}

/**
 * Apply one tool to the draft. **Total**: an unknown tool, or a selection outside the text,
 * returns the input unchanged rather than throwing — the value may arrive from a `data-tool`
 * attribute, and a gesture nobody can name must not be able to eat a draft.
 */
export function applyTool(
  text: string,
  selStart: number,
  selEnd: number,
  tool: string,
): ToolEdit {
  const start = clamp(Math.min(selStart, selEnd), text.length)
  const end = clamp(Math.max(selStart, selEnd), text.length)
  if (tool === 'bold' || tool === 'italic' || tool === 'code') {
    return inline(text, start, end, tool)
  }
  if (tool === 'link') return link(text, start, end)
  if (tool === 'heading' || tool === 'quote' || tool === 'bullet' || tool === 'ordered') {
    return lines(text, start, end, tool)
  }
  return { text, selStart: start, selEnd: end }
}

function clamp(n: number, max: number): number {
  if (!Number.isFinite(n) || n < 0) return 0
  return n > max ? max : Math.floor(n)
}

/* --- inline wraps -------------------------------------------------------------------------- */

function inline(text: string, start: number, end: number, tool: string): ToolEdit {
  const marker = INLINE_MARKER[tool] ?? ''
  const len = marker.length
  const selected = text.slice(start, end)

  // Unwrap, outside-in: the selection is the content and the markers sit just beyond it —
  // which is exactly the state a wrap below leaves, so press-press is identity.
  if (
    start >= len &&
    text.slice(start - len, start) === marker &&
    text.slice(end, end + len) === marker &&
    // `**` must not be read as two `*`s: an italic press inside a bold pair would strip it.
    !(tool === 'italic' && (text.slice(start - 2, start) === '**' || text.slice(end, end + 2) === '**'))
  ) {
    return {
      text: text.slice(0, start - len) + selected + text.slice(end + len),
      selStart: start - len,
      selEnd: end - len,
    }
  }

  // Unwrap, inside-out: the user selected the markers along with the word (`**bold**` whole).
  if (
    selected.length >= 2 * len &&
    selected.startsWith(marker) &&
    selected.endsWith(marker) &&
    !(tool === 'italic' && selected.startsWith('**') && selected.length >= 4)
  ) {
    const inner = selected.slice(len, selected.length - len)
    return {
      text: text.slice(0, start) + inner + text.slice(end),
      selStart: start,
      selEnd: start + inner.length,
    }
  }

  // Wrap. An empty selection gets a placeholder, selected, so the press is visible and the
  // next keystroke replaces it.
  const content = selected === '' ? (INLINE_PLACEHOLDER[tool] ?? '') : selected
  return {
    text: text.slice(0, start) + marker + content + marker + text.slice(end),
    selStart: start + len,
    selEnd: start + len + content.length,
  }
}

/* --- the link ------------------------------------------------------------------------------ */

const URLISH = /^https?:\/\/\S+$/

function link(text: string, start: number, end: number): ToolEdit {
  const selected = text.slice(start, end)
  /*
   * Three shapes, keyed off what was selected: a URL becomes the destination with the text
   * placeholder selected; prose becomes the text with the destination placeholder selected;
   * nothing gets both placeholders with the text one selected. In every arm the selection
   * lands on the half the user still has to supply, so they type it without a click.
   */
  if (URLISH.test(selected)) {
    const t = 'text'
    return {
      text: `${text.slice(0, start)}[${t}](${selected})${text.slice(end)}`,
      selStart: start + 1,
      selEnd: start + 1 + t.length,
    }
  }
  const label = selected === '' ? 'text' : selected
  const url = 'url'
  const made = `[${label}](${url})`
  const urlAt = start + 1 + label.length + 2
  if (selected === '') {
    return {
      text: text.slice(0, start) + made + text.slice(end),
      selStart: start + 1,
      selEnd: start + 1 + label.length,
    }
  }
  return {
    text: text.slice(0, start) + made + text.slice(end),
    selStart: urlAt,
    selEnd: urlAt + url.length,
  }
}

/* --- the line tools ------------------------------------------------------------------------ */

/*
 * `## ` rather than `# `: a task body or a comment is already inside a card with a heading
 * over it, so the level the button writes is a *section* of that, not a rival title. A user
 * who wants another level edits the text — the syntax the button spelled out is the lesson.
 */
const HEADING = '## '
const QUOTE = '> '
const BULLET = '- '

function lines(text: string, start: number, end: number, tool: string): ToolEdit {
  // Widen to whole lines: a line tool aimed anywhere in a line means the line.
  const from = text.lastIndexOf('\n', start - 1) + 1
  const endScan = text.indexOf('\n', end)
  const to = endScan === -1 ? text.length : endScan
  const block = text.slice(from, to)
  const rows = block.split('\n')

  // Numbered in the order the rows sit, so the button writes a list and not `1.` eight times.
  // Markdown renumbers anyway; the source should still read as what it means.
  let ordinal = 0
  const prefixed = rows
    .map((row) => {
      if (row.trim() === '' && rows.length > 1) return row
      ordinal += 1
      return transformRow(row, rows, tool, ordinal)
    })
    .join('\n')
  const next = text.slice(0, from) + prefixed + text.slice(to)
  return { text: next, selStart: from, selEnd: from + prefixed.length }
}

/*
 * A blank line inside the selection is a separator, not a candidate — the caller skips it
 * before this runs, so prefixing one can never mint an empty heading or a bare `-`.
 */
function transformRow(row: string, rows: readonly string[], tool: string, ordinal: number): string {
  if (tool === 'heading') {
    return rows.every(has(/^#{1,6} /)) ? row.replace(/^#{1,6} /, '') : HEADING + strip(row, tool)
  }
  if (tool === 'quote') {
    return rows.every(has(/^> /)) ? row.replace(/^> /, '') : QUOTE + strip(row, tool)
  }
  if (tool === 'bullet') {
    return rows.every(has(/^[-*] /)) ? row.replace(/^[-*] /, '') : BULLET + strip(row, tool)
  }
  return rows.every(has(/^\d+\. /)) ? row.replace(/^\d+\. /, '') : `${ordinal}. ` + strip(row, tool)
}

/* "Every non-blank row already carries the prefix" is what makes the press a toggle off. */
function has(re: RegExp): (row: string) => boolean {
  return (row) => row.trim() === '' || re.test(row)
}

/*
 * Switching families strips the old marker first — Bullet on a numbered line replaces the
 * number rather than stacking `- 1. `. Only the *other* families are stripped; the tool's own
 * marker is the toggle above.
 */
function strip(row: string, tool: string): string {
  let out = row
  if (tool !== 'heading') out = out.replace(/^#{1,6} /, '')
  if (tool !== 'quote') out = out.replace(/^> /, '')
  if (tool !== 'bullet') out = out.replace(/^[-*] /, '')
  if (tool !== 'ordered') out = out.replace(/^\d+\. /, '')
  return out
}
