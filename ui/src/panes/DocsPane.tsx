/**
 * A symbol's documentation, as one page for every language. (M60)
 *
 * # What this draws and what it does not decide
 *
 * A `SymbolDocs` — title, kind, a signature, a markdown body, a member list, links — filled in
 * Rust by whichever provider the owning server has (`cide_lsp::docs`: hover for any server,
 * Godot's native symbols for Godot). Nothing here knows which; the page is the contract, and a
 * Java server's javadoc will land in the same five places the moment an extension declares
 * one. The body is the markdown preview cide already has (`ExtensionTab` is the template: parse
 * a string, render it, decide what a link does).
 *
 * # Why the tab carries a subject and the pane asks
 *
 * `TabKind::Docs` holds what to *ask* and never the page — `TabKind::Docker`'s argument: a tab
 * is durable and a page is not. So this component asks on every mount, and asks again when a
 * reference inside the page is followed: a class page links to its members, each of which is a
 * page, and the trail is kept here so Back returns to the class. A followed reference stays in
 * this tab rather than opening another, because a class with a hundred members would otherwise
 * be a hundred tabs.
 *
 * # Links
 *
 * The preview writes no `href` and hands every activation to `onNavigate`. Three shapes: an
 * in-page reference (`#ref/method/Node.add_child`, a fragment, so it is never drawn with the
 * leave-the-app marker) is followed here; an `https://` link opens in the browser through the
 * same Rust-checked road an extension README uses; anything else is refused.
 */
import { useEffect, useMemo, useState } from 'react'

import { parseMarkdown } from '@/editor/markdown/blocks'
import { MarkdownPreview } from '@/editor/markdown/MarkdownPreview'
import { Icon, asIcon } from '@/icons/Icon'
import {
  docs as docsApi,
  ext as extApi,
  type DocsSubject,
  type ProjectId,
  type SymbolDocs,
} from '@/ipc/client'
import { errorText } from '@/ipc/errorText'

import styles from './DocsPane.module.css'

export interface DocsPaneProps {
  readonly project: ProjectId
  /** What the tab is about. The first page; following a reference moves on from it. */
  readonly subject: DocsSubject
  /** The stored tab title, drawn until the page has a better one. */
  readonly title: string
}

/**
 * The subject a `#ref/<kind>/<target>` link names on `source`, or null for any other link.
 * Mirrors `cide_lsp::docs::parse_reference`, so what Rust writes into a page is what this reads.
 */
export function referenceSubject(source: string, href: string): DocsSubject | null {
  if (!href.startsWith('#ref/')) return null
  const rest = href.slice('#ref/'.length)
  const slash = rest.indexOf('/')
  if (slash <= 0 || slash === rest.length - 1) return null
  return { kind: 'reference', source, refKind: rest.slice(0, slash), target: rest.slice(slash + 1) }
}

/** Put a sentence on screen — `goToDefinition.ts`'s channel, for its reason. */
function report(message: string): void {
  void Promise.reject(new Error(message))
}

export function DocsPane({ project, subject, title }: DocsPaneProps) {
  const [current, setCurrent] = useState<DocsSubject>(subject)
  const [trail, setTrail] = useState<readonly DocsSubject[]>([])
  const [page, setPage] = useState<SymbolDocs | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    setPage(null)
    setNotice(null)
    docsApi
      .page(project, current)
      .then((answer) => {
        if (!live) return
        switch (answer.kind) {
          case 'found':
            setPage(answer.page)
            return
          case 'notFound':
            setNotice('The language server has nothing to say about this.')
            return
          case 'unavailable':
            setNotice(answer.reason)
        }
      })
      .catch((error: unknown) => {
        if (live) setNotice(errorText(error))
      })
    return () => {
      live = false
    }
  }, [project, current])

  const body = useMemo(() => (page === null ? null : parseMarkdown(page.markdown)), [page])
  const memberDocs = useMemo(
    () => (page === null ? [] : page.members.map((member) => parseMarkdown(member.markdown))),
    [page],
  )

  const follow = (href: string): void => {
    if (page === null) return
    const next = referenceSubject(page.source, href)
    if (next !== null) {
      setTrail((previous) => [...previous, current])
      setCurrent(next)
      return
    }
    // The scheme is checked in Rust, not here — `ext.openLink`'s note. This only avoids a
    // round trip for a fragment or a relative path, which a page has no directory to resolve.
    if (href.includes('://')) {
      void extApi.openLink(href).catch((error: unknown) => report(errorText(error)))
    }
  }

  const back = (): void => {
    const previous = trail[trail.length - 1]
    if (previous === undefined) return
    setTrail(trail.slice(0, -1))
    setCurrent(previous)
  }

  return (
    <div className={styles.pane} data-audit="docsPane">
      <header className={styles.header}>
        <div className={styles.titleRow}>
          <button
            type="button"
            className={styles.back}
            onClick={back}
            disabled={trail.length === 0}
            aria-label="Back to the previous page"
            title="Back"
          >
            <Icon name={asIcon('chevron-left')} size={1} />
          </button>
          <span className={styles.mark} aria-hidden="true">
            <Icon name={asIcon('book-open-text')} size={2} />
          </span>
          <h1 className={styles.title}>{page?.title ?? title}</h1>
          {page !== null && <span className={styles.kind}>{page.kind}</span>}
          {page !== null && <span className={styles.source}>{page.source}</span>}
        </div>
        {page !== null && page.links.length > 0 && (
          <div className={styles.links}>
            {page.links.map((link) => (
              <button
                key={link.href}
                type="button"
                className={styles.link}
                onClick={() => follow(link.href)}
              >
                {link.label}
              </button>
            ))}
          </div>
        )}
        {page?.signature !== undefined && page.signature !== '' && (
          <pre className={styles.signature}>{page.signature}</pre>
        )}
      </header>
      <div className={styles.body}>
        {notice !== null ? (
          <p className={styles.notice}>{notice}</p>
        ) : page === null || body === null ? (
          // Blank rather than a spinner: one round trip, answered from memory for a page the
          // project has seen — `DockerInspectPane`'s reasoning.
          <div />
        ) : (
          <>
            <div className={styles.prose}>
              {page.markdown.trim() === '' ? (
                <p className={styles.empty}>No description.</p>
              ) : (
                <MarkdownPreview doc={body} path="" onNavigate={follow} scrolls={false} />
              )}
            </div>
            {page.members.length > 0 && (
              <section className={styles.members}>
                <h2 className={styles.membersTitle}>Members</h2>
                {page.members.map((member, at) => {
                  const doc = memberDocs[at]
                  return (
                    <div key={`${member.kind}:${member.name}`} className={styles.member}>
                      <div className={styles.memberHead}>
                        <button
                          type="button"
                          className={styles.memberName}
                          disabled={member.target === undefined}
                          onClick={() => {
                            if (member.target !== undefined) follow(member.target)
                          }}
                        >
                          {member.name}
                        </button>
                        <span className={styles.kind}>{member.kind}</span>
                      </div>
                      {member.signature !== undefined && member.signature !== '' && (
                        <pre className={styles.memberSignature}>{member.signature}</pre>
                      )}
                      {doc !== undefined && member.markdown.trim() !== '' && (
                        <MarkdownPreview doc={doc} path="" onNavigate={follow} scrolls={false} />
                      )}
                    </div>
                  )
                })}
              </section>
            )}
          </>
        )}
      </div>
    </div>
  )
}
