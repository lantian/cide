/**
 * What a parsed markdown document is, as data. (M20)
 *
 * # Why there is a tree at all, rather than a string of HTML
 *
 * Every markdown renderer in the ecosystem answers `string → HTML string`, and consuming one
 * means `dangerouslySetInnerHTML` plus a sanitizer, which is a dependency pair this project has
 * twice written comments to avoid: `panes/ImagePane.tsx` on why an SVG goes through `<img>` and
 * is never inlined, and `sidebar/TasksPanel/TaskDetail.tsx` on why a comment is `pre-wrap` text
 * and "never as HTML or markdown — it is model-authored, and rendering model-authored markup
 * inside the IDE's own chrome is an injection surface bought for nothing". A `.md` in a cloned
 * repository is exactly as untrusted as either.
 *
 * A tree closes that off structurally rather than by filtering. There is no node here that
 * carries markup, so there is no path from a document to script — not a sanitized one, not one
 * at all. `MarkdownPreview.tsx` maps these nodes to React elements and React escapes text.
 *
 * The second reason is that a tree can be *checked*. `ui/scripts/check-markdown.mjs` compiles
 * this module and its two parsers with the TypeScript in `node_modules` and asserts on the
 * values they produce, which is the only kind of frontend test this repository has. An HTML
 * string would be assertable only by matching substrings of it.
 *
 * Import-free, and deliberately so — see the compile list in `check-markdown.mjs`.
 */

/** The three layouts a markdown pane can be in. Mirrors Rust's `MarkdownView`. */
export type MdView = 'text' | 'split' | 'preview'

/* --- inline ------------------------------------------------------------------------------- */

/**
 * A span inside a paragraph, heading or table cell.
 *
 * `text` is the only leaf that carries characters the user wrote, and it is rendered as a text
 * node. There is deliberately no `html` variant: raw HTML in the source becomes `text`, so
 * `<b>hi</b>` in a `.md` shows those eight characters. That is a real difference from GitHub and
 * it is the point — see the header.
 */
export type Inline =
  | { readonly kind: 'text'; readonly text: string }
  /** A code span. Never re-parsed, never highlighted; backticks are a quoting device. */
  | { readonly kind: 'code'; readonly text: string }
  | { readonly kind: 'strong'; readonly body: readonly Inline[] }
  | { readonly kind: 'em'; readonly body: readonly Inline[] }
  | { readonly kind: 'del'; readonly body: readonly Inline[] }
  | {
      readonly kind: 'link'
      readonly href: string
      readonly title: string | null
      readonly body: readonly Inline[]
    }
  /**
   * `![alt](src)`.
   *
   * `alt` is a flat string rather than an `Inline[]`: it ends up in an `alt=` attribute, which
   * is text, and a nested tree there would have to be flattened at render time anyway.
   */
  | { readonly kind: 'image'; readonly src: string; readonly title: string | null; readonly alt: string }
  /** A hard break — two trailing spaces, or a trailing backslash. */
  | { readonly kind: 'break' }

/* --- blocks ------------------------------------------------------------------------------- */

/** Column alignment from a GFM delimiter row. `null` is "not stated". */
export type Align = 'left' | 'center' | 'right' | null

export interface ListItem {
  /** 1-based source line the item's marker is on. */
  readonly line: number
  /**
   * `null` unless the item opened with `[ ]` or `[x]`.
   *
   * Rendered as a disabled checkbox. It is not clickable, and that is not an oversight: a click
   * would have to edit the buffer, and a preview that writes to the document the user is editing
   * is a second author of that file. `Ctrl+.`-style buffer edits belong to the buffer.
   */
  readonly checked: boolean | null
  readonly body: readonly Block[]
}

export interface TableRow {
  readonly line: number
  readonly cells: readonly (readonly Inline[])[]
}

/**
 * A top-level or nested block.
 *
 * **Every block carries `line`**, the 1-based source line it starts on, and that is not
 * bookkeeping — it is the whole of what makes split-mode scroll sync possible. `scrollSync.ts`
 * builds its anchor table out of these numbers, for the reason `crates/cide-ipc/src/positions.rs`
 * gives under *Why lines and not pixels*: a line survives a resize, a font change and a sidebar
 * drag, and a pixel offset does not.
 */
export type Block =
  | { readonly kind: 'heading'; readonly line: number; readonly level: 1 | 2 | 3 | 4 | 5 | 6; readonly body: readonly Inline[] }
  | { readonly kind: 'paragraph'; readonly line: number; readonly body: readonly Inline[] }
  /** A fenced or indented code block. `lang` is the fence's info word, lowercased. */
  | { readonly kind: 'code'; readonly line: number; readonly lang: string | null; readonly text: string }
  | { readonly kind: 'quote'; readonly line: number; readonly body: readonly Block[] }
  | {
      readonly kind: 'list'
      readonly line: number
      readonly ordered: boolean
      /** The first number of an ordered list; 1 for a bullet list. */
      readonly start: number
      /** A tight list renders its items without paragraph spacing. */
      readonly tight: boolean
      readonly items: readonly ListItem[]
    }
  | { readonly kind: 'rule'; readonly line: number }
  | {
      readonly kind: 'table'
      readonly line: number
      readonly head: readonly (readonly Inline[])[]
      readonly align: readonly Align[]
      readonly rows: readonly TableRow[]
    }

/** What a link reference definition (`[id]: url "title"`) resolves to. */
export interface LinkDef {
  readonly href: string
  readonly title: string | null
}

/** A whole parsed document. */
export interface MarkdownDoc {
  readonly blocks: readonly Block[]
  /** Every block's source line, ascending — the anchor keys, precomputed once. */
  readonly lines: readonly number[]
}
