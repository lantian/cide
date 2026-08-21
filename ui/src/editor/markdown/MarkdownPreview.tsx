/**
 * The rendered half of a markdown pane. (M20)
 *
 * Blocks in, React elements out. There is no `dangerouslySetInnerHTML` here and there is no way
 * to add one usefully: `blocks.ts` produces no node that carries markup, so every string that
 * reaches the DOM goes through React's text escaping. See `types.ts` for the argument.
 *
 * # What this component does not do
 *
 * It does not scroll itself, measure itself, or know which layout the pane is in. The host
 * (`MarkdownFrame.tsx`) owns the scroll container's ref, the resize observer and the sync latch,
 * because those are questions about *two* surfaces and this one only knows about one.
 *
 * # Every top-level block carries `data-line`
 *
 * That attribute is the entire scroll-sync mechanism — `MarkdownFrame` reads it back off the DOM
 * to build the anchor table `scrollSync.ts` interpolates over. Nested blocks deliberately do not
 * carry one: an anchor per list item would be an anchor every three lines, and the table is
 * walked on every frame of a scroll.
 */
import { useEffect, useMemo, useReducer, useSyncExternalStore, type JSX, type ReactNode } from 'react'
import { TOKEN_VAR_BY_CLASS } from '../highlight'
import { fenceIsHighlightable, fenceLanguage, grammarIfReady, requestGrammar, tokenizeFence } from './fenceTokens'
import { imageState, requestImage, subscribeImages } from './images'
import { dirOf, looksLikeImage, resolveLocal, targetKind, uniqueSlug } from './links'
import type { Align, Block, Inline, MarkdownDoc } from './types'
import styles from './MarkdownPreview.module.css'

export interface MarkdownPreviewProps {
  /** The parsed document. Parsing is the host's job, so that it can debounce it. */
  doc: MarkdownDoc
  /** The markdown file's own absolute path, for resolving relative targets. */
  path: string
  /** Activating a link. The host decides what a local, external or fragment target does. */
  onNavigate: (href: string) => void
}

/** What the recursive renderers need that is not a block. */
interface Ctx {
  readonly dir: string
  readonly slugs: Map<string, number>
  readonly onNavigate: (href: string) => void
}

export function MarkdownPreview({ doc, path, onNavigate }: MarkdownPreviewProps): JSX.Element {
  /*
   * A fresh slug map per document, and per *render* of that document.
   *
   * `uniqueSlug` mutates it, so reusing one across renders would make the same heading `intro`,
   * then `intro-1`, then `intro-2` — an anchor that changes every time React re-runs the tree,
   * which is exactly the class of link that works when you test it and not afterwards.
   */
  const ctx = useMemo<Ctx>(
    () => ({ dir: dirOf(path), slugs: new Map<string, number>(), onNavigate }),
    [path, onNavigate, doc],
  )

  return (
    <>
      {doc.blocks.map((block, index) => (
        <div key={`${block.line}:${index}`} className={styles.anchor} data-line={block.line}>
          {renderBlock(block, ctx, `${index}`)}
        </div>
      ))}
    </>
  )
}

/* --- blocks -------------------------------------------------------------------------------- */

function renderBlocks(blocks: readonly Block[], ctx: Ctx, key: string): ReactNode[] {
  return blocks.map((block, index) => (
    <Fragmentish key={`${key}.${index}`}>{renderBlock(block, ctx, `${key}.${index}`)}</Fragmentish>
  ))
}

/** A keyed passthrough, so `renderBlocks` can key children that are not elements it made. */
function Fragmentish({ children }: { children: ReactNode }): JSX.Element {
  return <>{children}</>
}

function renderBlock(block: Block, ctx: Ctx, key: string): ReactNode {
  switch (block.kind) {
    case 'heading': {
      const id = uniqueSlug(plainText(block.body), ctx.slugs)
      const props = { id, className: styles.heading }
      const body = renderInline(block.body, ctx, key)
      if (block.level === 1) return <h1 {...props}>{body}</h1>
      if (block.level === 2) return <h2 {...props}>{body}</h2>
      if (block.level === 3) return <h3 {...props}>{body}</h3>
      if (block.level === 4) return <h4 {...props}>{body}</h4>
      if (block.level === 5) return <h5 {...props}>{body}</h5>
      return <h6 {...props}>{body}</h6>
    }
    case 'paragraph':
      return <p className={styles.paragraph}>{renderInline(block.body, ctx, key)}</p>
    case 'rule':
      return <hr className={styles.rule} />
    case 'code':
      return <Fence lang={block.lang} text={block.text} />
    case 'quote':
      return <blockquote className={styles.quote}>{renderBlocks(block.body, ctx, key)}</blockquote>
    case 'list': {
      const items = block.items.map((item, index) => (
        <li
          key={`${key}.i${index}`}
          className={item.checked === null ? styles.item : styles.taskItem}
        >
          {item.checked === null ? null : (
            <input
              type="checkbox"
              className={styles.task}
              checked={item.checked}
              readOnly
              /*
               * Disabled *and* read-only. A click would have to edit the buffer, and a preview
               * that writes to the document the user is editing is a second author of that file
               * — the buffer is one click away in the same control and owns every edit.
               */
              disabled
              aria-label={item.checked ? 'done' : 'not done'}
            />
          )}
          {renderBlocks(item.body, ctx, `${key}.i${index}`)}
        </li>
      ))
      const className = block.tight ? styles.tight : styles.loose
      return block.ordered ? (
        <ol className={className} start={block.start}>
          {items}
        </ol>
      ) : (
        <ul className={className}>{items}</ul>
      )
    }
    case 'table':
      return (
        /*
         * The wrapper is not decoration. A table wider than the pane must scroll *itself*; the
         * alternative is a preview whose body scrolls sideways, which moves the prose out from
         * under the reader to show them a column of a table they were not looking at.
         */
        <div className={styles.tableWrap}>
          <table className={styles.table}>
            <thead>
              <tr>
                {block.head.map((cell, index) => (
                  <th key={`${key}.h${index}`} style={alignOf(block.align[index] ?? null)}>
                    {renderInline(cell, ctx, `${key}.h${index}`)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, r) => (
                <tr key={`${key}.r${r}`}>
                  {row.cells.map((cell, c) => (
                    <td key={`${key}.r${r}c${c}`} style={alignOf(block.align[c] ?? null)}>
                      {renderInline(cell, ctx, `${key}.r${r}c${c}`)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )
  }
}

function alignOf(align: Align): { textAlign: 'left' | 'center' | 'right' } | undefined {
  return align === null ? undefined : { textAlign: align }
}

/* --- inline -------------------------------------------------------------------------------- */

function renderInline(nodes: readonly Inline[], ctx: Ctx, key: string): ReactNode[] {
  return nodes.map((node, index) => {
    const k = `${key}.${index}`
    switch (node.kind) {
      case 'text':
        return <span key={k}>{node.text}</span>
      case 'code':
        return (
          <code key={k} className={styles.codeSpan}>
            {node.text}
          </code>
        )
      case 'strong':
        return <strong key={k}>{renderInline(node.body, ctx, k)}</strong>
      case 'em':
        return <em key={k}>{renderInline(node.body, ctx, k)}</em>
      case 'del':
        return <del key={k}>{renderInline(node.body, ctx, k)}</del>
      case 'break':
        return <br key={k} />
      case 'image':
        return <PreviewImage key={k} src={node.src} alt={node.alt} title={node.title} dir={ctx.dir} />
      case 'link':
        return (
          <Link key={k} target={node.href} title={node.title} onNavigate={ctx.onNavigate}>
            {renderInline(node.body, ctx, k)}
          </Link>
        )
    }
  })
}

/** The characters of an inline run, for a slug or an `alt`. */
function plainText(nodes: readonly Inline[]): string {
  let out = ''
  for (const node of nodes) {
    switch (node.kind) {
      case 'text':
      case 'code':
        out += node.text
        break
      case 'strong':
      case 'em':
      case 'del':
      case 'link':
        out += plainText(node.body)
        break
      case 'image':
        out += node.alt
        break
      case 'break':
        out += ' '
        break
    }
  }
  return out
}

/**
 * A link, which is deliberately not an `<a href>`.
 *
 * There is no `href` anywhere in this component, on any kind of target. An `<a href="https://…">`
 * in a Tauri webview is a way to navigate **the application itself** away from its own document —
 * `terminal/xterm.ts` has the write-up of the same hazard arriving through OSC 8, where until it
 * was closed "any program in any pane could emit `ESC ] 8 ;; https://… ST` and a single click on
 * the text it wrapped would navigate part of the application to a URL that program chose". A
 * middle-click, a Ctrl+click or a `preventDefault` that one code path forgets is all it takes,
 * so the attribute is simply never written and there is nothing to forget.
 *
 * `role="link"` and `tabIndex` put it back in the accessibility tree and on the tab order, and
 * Enter and Space activate it, which is what an `<a>` would have given for free.
 */
function Link({
  target,
  title,
  onNavigate,
  children,
}: {
  /**
   * Where the link points.
   *
   * Called `target` and not `href` so that the absence of the DOM attribute is visible at the one
   * call site as well as here — `ui/scripts/check-markdown.mjs` asserts that the string `href=`
   * appears nowhere in this file, and a prop of that name would satisfy the letter of the rule
   * while doing nothing about the reason behind it.
   */
  target: string
  title: string | null
  onNavigate: (href: string) => void
  children: ReactNode
}): JSX.Element {
  return (
    <span
      className={styles.link}
      role="link"
      tabIndex={0}
      title={title ?? target}
      data-target={targetKind(target)}
      onClick={() => {
        onNavigate(target)
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return
        event.preventDefault()
        onNavigate(target)
      }}
    >
      {children}
    </span>
  )
}

/**
 * An image, or an honest account of why there isn't one.
 *
 * Three outcomes and all three are visible. A local file that Rust vouched for draws. A local
 * file Rust refused draws its alt text and the refusal on hover — that sentence is the whole
 * value of `image_read` rejecting rather than serving. A remote URL draws its alt text and says
 * so, because the CSP has no `https:` in `img-src` and never will: fetching a URL out of a
 * document is a request the document's author chose to make from the user's machine.
 */
function PreviewImage({
  src,
  alt,
  title,
  dir,
}: {
  src: string
  alt: string
  title: string | null
  dir: string
}): JSX.Element {
  const kind = targetKind(src)
  const local = kind === 'local' ? resolveLocal(dir, src) : null
  const wanted = local !== null && looksLikeImage(local) ? local : null

  const state = useSyncExternalStore(
    subscribeImages,
    () => (wanted === null ? undefined : imageState(wanted)),
    () => undefined,
  )

  useEffect(() => {
    if (wanted !== null) requestImage(wanted)
  }, [wanted])

  if (wanted === null) {
    const why =
      kind === 'local'
        ? 'not an image cide can display'
        : 'cide does not fetch remote images into a preview'
    return (
      <span className={styles.imageMissing} title={`${src} — ${why}`}>
        {alt === '' ? src : alt}
      </span>
    )
  }
  if (state === undefined) return <span className={styles.imagePending}>{alt}</span>
  if ('refused' in state) {
    return (
      <span className={styles.imageMissing} title={`${src} — ${state.refused}`}>
        {alt === '' ? src : alt}
      </span>
    )
  }
  return <img className={styles.image} src={state.url} alt={alt} title={title ?? undefined} />
}

/**
 * A fenced code block, coloured by the buffer's own grammar once its chunk has arrived.
 *
 * Uncoloured first and coloured a tick later, never the other way round and never blank in
 * between: `fenceTokens.ts::requestGrammar` takes a callback rather than returning a promise
 * precisely so this component can render something on the first frame. A language whose chunk
 * fails to load simply stays uncoloured, which is what the buffer shows for a fence anyway.
 */
function Fence({ lang, text }: { lang: string | null; text: string }): JSX.Element {
  const id = fenceLanguage(lang)
  const [, bump] = useReducer((n: number) => n + 1, 0)
  const spec = id === null ? null : grammarIfReady(id)

  useEffect(() => {
    if (id !== null && spec === null) requestGrammar(id, bump)
  }, [id, spec])

  const lines = useMemo(
    () => (spec === null || !fenceIsHighlightable(text) ? null : tokenizeFence(spec, text)),
    [spec, text],
  )

  return (
    <pre className={styles.code} data-lang={lang ?? undefined}>
      <code>
        {lines === null
          ? text
          : lines.map((tokens, index) => (
              <span key={index} className={styles.codeLine}>
                {tokens.map((token, at) =>
                  token.cls === null ? (
                    <span key={at}>{token.text}</span>
                  ) : (
                    <span key={at} style={{ color: `var(${TOKEN_VAR_BY_CLASS.get(token.cls) ?? '--text'})` }}>
                      {token.text}
                    </span>
                  ),
                )}
                {index === lines.length - 1 ? null : '\n'}
              </span>
            ))}
      </code>
    </pre>
  )
}
