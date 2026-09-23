import { useMemo } from 'react'
import { Marked } from 'marked'
import { useGitLab } from './store'
import DOMPurify from 'dompurify'
import { gitlab } from '@/ipc/client'
import { TaskMarkdown } from '@/sidebar/TasksPanel/TaskMarkdown'
import styles from './GitLab.module.css'

/** GitLab notes contain Markdown and inline HTML. Sanitize after parsing, before insertion. */
export function Markdown({
  text,
  baseUrl,
  review,
}: {
  text: string
  baseUrl?: string | undefined
  review?: string | undefined
}) {
  const { board } = useGitLab()
  const account = board.reviews.find((r) => r.id === review)?.account
  const host = board.accounts.find((a) => a.id === account)?.host
  const html = useMemo(() => {
    if (typeof DOMPurify.sanitize !== 'function') return null
    const parser = new Marked({ async: false, breaks: true, gfm: true })
    if (host)
      parser.use({
        extensions: [
          {
            name: 'gitlabMention',
            level: 'inline',
            start: (source) => source.indexOf('@'),
            tokenizer(source) {
              const match =
                /^@([a-zA-Z0-9_][a-zA-Z0-9_.-]*[a-zA-Z0-9_]|[a-zA-Z0-9_])/.exec(
                  source,
                )
              if (match)
                return {
                  type: 'gitlabMention',
                  raw: match[0],
                  username: match[1],
                }
            },
            renderer(token) {
              const username = String(token.username)
              const href =
                `${host.replace(/\/$/, '')}/${encodeURIComponent(username)}`
                  .replaceAll('&', '&amp;')
                  .replaceAll('"', '&quot;')
                  .replaceAll('<', '&lt;')
              return `<a href="${href}">@${username}</a>`
            },
          },
        ],
      })
    return DOMPurify.sanitize(parser.parse(text, { async: false }), {
      ALLOWED_TAGS: [
        'p',
        'br',
        'strong',
        'b',
        'em',
        'i',
        's',
        'del',
        'a',
        'pre',
        'code',
        'blockquote',
        'ul',
        'ol',
        'li',
        'hr',
        'h1',
        'h2',
        'h3',
        'h4',
        'h5',
        'h6',
        'table',
        'thead',
        'tbody',
        'tr',
        'th',
        'td',
        'details',
        'summary',
        'sup',
        'sub',
        'input',
      ],
      ALLOWED_ATTR: [
        'href',
        'title',
        'start',
        'align',
        'type',
        'checked',
        'disabled',
      ],
      ALLOW_DATA_ATTR: false,
    })
  }, [text, host])
  // The headless renderer has no HTML parser; keep its output safe as React text/Markdown.
  if (html === null)
    return (
      <div className={styles.markdown}>
        <TaskMarkdown text={text} />
      </div>
    )
  return (
    <div
      className={styles.markdown}
      onClick={(e) => {
        const link = (e.target as Element).closest('a')
        if (!link) return
        e.preventDefault()
        try {
          const url = new URL(link.getAttribute('href') ?? '', baseUrl)
          if (url.protocol === 'https:') void gitlab.openUrl(url.href)
        } catch {
          /* Invalid links stay inert. */
        }
      }}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
