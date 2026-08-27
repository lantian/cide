/**
 * A task's body or comment, rendered as markdown. (M27)
 *
 * # Why this exists when `MarkdownPreview.tsx` already renders markdown
 *
 * The preview is a *pane*: its fences are tokenized by driving the language's real
 * `StreamParser` (a CodeMirror import the render check must never see in this panel's bundle),
 * its images go through an IPC fetch with an LRU, and its links resolve against a file's own
 * directory. A comment in a 620px card needs none of that, and dragging it in would couple the
 * tracker to the editor stack. What **is** shared is the parser — `editor/markdown/blocks.ts` —
 * because two grammars would disagree on the first nested list somebody wrote, and that pair of
 * files is import-free TypeScript with no dependency to inherit.
 *
 * # Safe by construction, which is what unlocked rendering at all
 *
 * `TaskDetail.tsx` refused markup here for years on injection grounds, and the refusal was
 * about `string → HTML string` renderers: model-authored text into `dangerouslySetInnerHTML`.
 * That road is not taken. The parser produces **no HTML node of any kind** — `<b>` in a comment
 * is four characters of text (`types.ts` carries the argument) — and every string below reaches
 * the DOM as a React child, through React's own escaping. There is no `dangerouslySetInnerHTML`
 * in this file and nothing for one to render.
 *
 * # Links are inert, and that is a decision
 *
 * A markdown link draws as accented text with the destination in its `title`, and activates
 * nothing. The card reads no store and calls no IPC (its header's rule), there is no
 * URL-opening command on the wire today, and a model-authored `[innocent words](anywhere)` that
 * navigated on click inside the IDE's own chrome is exactly the surface the old refusal was
 * guarding. Showing the destination while refusing to follow it is the honest half of a link.
 * An image is the same shape one step further: its alt text, marked, fetching nothing.
 */
import { useMemo, type JSX, type ReactNode } from 'react'
import { parseMarkdown } from '@/editor/markdown/blocks'
import type { Block, Inline, ListItem } from '@/editor/markdown/types'
import { Icon } from '@/icons/Icon'

import styles from './TasksPanel.module.css'

export function TaskMarkdown({ text }: { text: string }): JSX.Element {
  /*
   * Memoised on the text: the card re-renders on every keystroke of an *unrelated* field's
   * draft, and re-parsing a long body for each is work the identity of the string can skip.
   *
   * `softBreak: 'break'` is the one place this renderer diverges from `MarkdownPreview.tsx`, and
   * the divergence is deliberate — do not "unify" them. The preview renders a `.md` *file*, where
   * CommonMark is right: the author wrapped at 100 columns and meant nothing by it. A task body
   * or a comment is a *message*, written in a textarea and read at 620px, and every comment box a
   * person has ever used treats a newline as a line. Without this, an agent reporting five
   * numbered steps one per line is drawn as one wall of prose — which is what it was.
   */
  const doc = useMemo(() => parseMarkdown(text, { softBreak: 'break' }), [text])
  return <>{blocks(doc.blocks, 'b')}</>
}

function blocks(list: readonly Block[], key: string): ReactNode[] {
  return list.map((block, index) => renderBlock(block, `${key}.${index}`))
}

function renderBlock(block: Block, key: string): ReactNode {
  switch (block.kind) {
    case 'heading': {
      /*
       * Real `h1`–`h6`, capped by the stylesheet to two card-scale sizes rather than the
       * document scale — an `#` in a comment is a section of that comment, not a rival to the
       * dialog's own title. The tag still carries the level for a screen reader's outline.
       */
      const body = inlines(block.body, key)
      const cls = styles.mdHeading
      if (block.level === 1) return <h1 key={key} className={cls}>{body}</h1>
      if (block.level === 2) return <h2 key={key} className={cls}>{body}</h2>
      if (block.level === 3) return <h3 key={key} className={cls}>{body}</h3>
      if (block.level === 4) return <h4 key={key} className={cls}>{body}</h4>
      if (block.level === 5) return <h5 key={key} className={cls}>{body}</h5>
      return <h6 key={key} className={cls}>{body}</h6>
    }
    case 'paragraph':
      return <p key={key} className={styles.mdP}>{inlines(block.body, key)}</p>
    case 'code':
      // Uncoloured on purpose: tokenizing needs the grammar registry, and a comment's fence is
      // read for its shape. The `<code>` inside keeps copy-paste and AT semantics.
      return (
        <pre key={key} className={styles.mdPre}>
          <code>{block.text}</code>
        </pre>
      )
    case 'quote':
      return <blockquote key={key} className={styles.mdQuote}>{blocks(block.body, key)}</blockquote>
    case 'list': {
      const items = block.items.map((item, index) => renderItem(item, `${key}.${index}`))
      return block.ordered ? (
        <ol key={key} className={styles.mdList} start={block.start}>{items}</ol>
      ) : (
        <ul key={key} className={styles.mdList}>{items}</ul>
      )
    }
    case 'rule':
      return <hr key={key} className={styles.mdRule} />
    case 'table':
      /*
       * `display: block` + `overflow-x: auto` live on the table's own class, so a wide one
       * scrolls inside the card rather than widening it — the app-wide rule about wide content,
       * without wrapping a `<div>` this small renderer would be the only user of.
       */
      return (
        <table key={key} className={styles.mdTable}>
          <thead>
            <tr>
              {block.head.map((cell, index) => (
                <th key={index} align={block.align[index] ?? undefined}>
                  {inlines(cell, `${key}.h${index}`)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {block.rows.map((row, r) => (
              <tr key={r}>
                {row.cells.map((cell, c) => (
                  <td key={c} align={block.align[c] ?? undefined}>
                    {inlines(cell, `${key}.${r}.${c}`)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      )
  }
}

function renderItem(item: ListItem, key: string): ReactNode {
  /*
   * A task item's box is an `Icon`, never an `<input type="checkbox">`. Partly `ListItem`'s own
   * argument (a click would have to edit a file this card is only reading), and partly this
   * panel's render gate: `check:agents-render` counts `<input>`s inside field rows to prove the
   * card is read-only at rest, and a checkbox in a body would indict the card it sits in.
   */
  return (
    <li key={key} className={item.checked === null ? styles.mdItem : styles.mdTask}>
      {item.checked !== null && (
        <span className={styles.mdCheck} data-checked={item.checked} aria-hidden="true">
          <Icon name={item.checked ? 'square-check-big' : 'square'} size={1} />
        </span>
      )}
      {blocks(item.body, key)}
    </li>
  )
}

function inlines(list: readonly Inline[], key: string): ReactNode[] {
  return list.map((node, index) => renderInline(node, `${key}.${index}`))
}

function renderInline(node: Inline, key: string): ReactNode {
  switch (node.kind) {
    case 'text':
      return node.text
    case 'code':
      return <code key={key} className={styles.mdCode}>{node.text}</code>
    case 'strong':
      return <strong key={key}>{inlines(node.body, key)}</strong>
    case 'em':
      return <em key={key}>{inlines(node.body, key)}</em>
    case 'del':
      return <del key={key}>{inlines(node.body, key)}</del>
    case 'link':
      // Inert; see the header. The destination is in `title`, so hovering answers "where".
      return (
        <span key={key} className={styles.mdLink} title={node.href}>
          {inlines(node.body, key)}
        </span>
      )
    case 'image':
      return (
        <span key={key} className={styles.mdLink} title={node.src}>
          {node.alt !== '' ? node.alt : node.src}
        </span>
      )
    case 'break':
      return <br key={key} />
  }
}
