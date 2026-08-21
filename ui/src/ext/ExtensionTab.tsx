/**
 * One extension's page: its README, what it contributes, and what it asks for. (M22)
 *
 * Renders over a tab's pane tree exactly as `SettingsTab` does, and for the same reason: it is a
 * page *about* cide rather than a document in the project.
 *
 * # Why the README is not just opened as a file
 *
 * Because the file is not in the project. An installed extension lives under `$XDG_STATE_HOME`
 * and one that is merely listed lives in a marketplace clone — both outside every root, which is
 * exactly what `cmd::file`'s refusals exist to keep out of an editor. Widening that for this
 * would widen it for everything.
 *
 * It also has to draw more than a file. A person reading an extension's README is deciding
 * whether to install it, so the version, the permissions it asks for and the button are on the
 * same page as the prose rather than back in the panel.
 *
 * # The markdown is cide's own
 *
 * `parseMarkdown` and `MarkdownPreview`, the same pair the preview pane uses. That is a render of
 * *somebody else's* markdown, which is worth being precise about: the preview renders no raw HTML
 * — `blocks.ts` has no html-block production and `inline.ts` escapes what it does not understand —
 * so a README is prose, links and code, and nothing it contains becomes markup. Links go through
 * `onNavigate`, which here opens external ones and refuses relative ones, because a relative link
 * in this README points into a directory the user has no project open on.
 */
import { useEffect, useMemo, useState } from 'react'

import { notifyFailure } from '@/chrome/notices'
import { ext as extApi } from '@/ipc/client'
import type { Capability, ExtensionPage, ExtensionRef } from '@/ipc/client'
import { parseMarkdown } from '@/editor/markdown/blocks'
import { MarkdownPreview } from '@/editor/markdown/MarkdownPreview'
import { useExtStore } from './extStore'
import styles from './ExtensionTab.module.css'
import { CAPABILITY_PROSE } from '@/sidebar/ExtensionsPanel/model'

export interface ExtensionTabProps {
  readonly id: ExtensionRef
  /** The caption the tab was opened with, so the header has one before the page arrives. */
  readonly name: string
}

export function ExtensionTab({ id, name }: ExtensionTabProps): React.JSX.Element {
  const [page, setPage] = useState<ExtensionPage | null>(null)
  const [failed, setFailed] = useState<string | null>(null)
  const run = useExtStore((state) => state.run)
  const busy = useExtStore((state) => state.busy)
  // The registry's revision, not the snapshot: this component does not read the snapshot, it
  // re-fetches its own page when the registry moves. Depending on the whole object would re-fetch
  // on every unrelated change, and depending on nothing would leave the page saying "Install"
  // after the user installed it from here.
  const rev = useExtStore((state) => state.snapshot.rev)

  useEffect(() => {
    let live = true
    void extApi
      .page(id)
      .then((answer) => {
        if (live) setPage(answer)
      })
      .catch((error: unknown) => {
        if (live) setFailed(String(error))
      })
    return () => {
      live = false
    }
  }, [id.marketplace, id.extension, rev])

  const doc = useMemo(
    () => (page?.readme === undefined ? null : parseMarkdown(page.readme)),
    [page?.readme],
  )

  const entry = page?.entry
  const installed = page?.installed
  const capabilities = entry?.capabilities ?? installed?.capabilities ?? []
  const version = entry?.version ?? installed?.version ?? ''

  const guarded = (work: () => Promise<unknown>): void => {
    void run(work as () => Promise<never>).catch(notifyFailure)
  }

  return (
    <div className={styles.tab}>
      <header className={styles.header}>
        <div className={styles.titleRow}>
          <h1 className={styles.title}>{entry?.name ?? installed?.name ?? name}</h1>
          {version !== '' && <span className={styles.version}>{version}</span>}
          <span className={styles.source} title={page?.readmePath ?? undefined}>
            {id.marketplace}
          </span>
        </div>
        {(entry?.description ?? '') !== '' && (
          <p className={styles.description}>{entry?.description}</p>
        )}

        {capabilities.length > 0 && (
          <ul className={styles.capabilities}>
            {/*
             * One line per permission, in words. `process:spawn` tells a user nothing; "run
             * programs on your machine" tells them what they are deciding. The wording is
             * `Capability::describe`'s, reached through the panel's `CAPABILITY_PROSE`, which
             * `check-ext.mjs` pins against the Rust it came from — a page that described a
             * permission differently from the permission it grants would be the worst possible
             * drift in this feature.
             */}
            {capabilities.map((cap) => (
              <li key={cap} className={styles.capability}>
                {CAPABILITY_PROSE[cap] ?? cap}
              </li>
            ))}
          </ul>
        )}

        <div className={styles.actions}>
          {entry !== undefined && installed === undefined && (
            <button
              type="button"
              className={styles.primary}
              disabled={busy || entry.unavailable !== undefined}
              title={entry.unavailable}
              onClick={() => guarded(() => extApi.install(id, capabilities as Capability[]))}
            >
              Install
            </button>
          )}
          {entry !== undefined && installed !== undefined && entry.updateAvailable && (
            <button
              type="button"
              className={styles.primary}
              disabled={busy}
              onClick={() => guarded(() => extApi.install(id, capabilities as Capability[]))}
            >
              Update to {entry.version}
            </button>
          )}
          {installed !== undefined && (
            <>
              <button
                type="button"
                className={styles.action}
                disabled={busy}
                onClick={() => guarded(() => extApi.setEnabled(id, !installed.enabled))}
              >
                {installed.enabled ? 'Disable' : 'Enable'}
              </button>
              <button
                type="button"
                className={styles.action}
                disabled={busy}
                onClick={() => guarded(() => extApi.uninstall(id))}
              >
                Remove
              </button>
            </>
          )}
        </div>

        {installed?.unavailable !== undefined && (
          <p className={styles.problem}>{installed.unavailable}</p>
        )}
        {entry === undefined && installed === undefined && page !== null && (
          // A tab restored from the reopen stack after its marketplace was disconnected. It says
          // so rather than closing itself: a tab that vanished when it was activated would be
          // worse than one that explains.
          <p className={styles.problem}>
            This extension is no longer listed by any connected marketplace, and is not installed.
          </p>
        )}
      </header>

      <div className={styles.body}>
        {failed !== null ? (
          <p className={styles.notice}>Could not read this extension: {failed}</p>
        ) : page === null ? (
          <p className={styles.notice}>Reading…</p>
        ) : doc === null ? (
          <p className={styles.notice}>This extension ships no README.</p>
        ) : (
          <div className={styles.markdown}>
            <MarkdownPreview
              doc={doc}
              path={page.readmePath ?? ''}
              onNavigate={(href) => {
                // External links open in the user's browser. Everything else — a relative path, a
                // fragment — is refused rather than guessed at: a relative link in this README
                // points into a directory under `$XDG_STATE_HOME` that the user has no project
                // open on, and opening it would either fail or reveal a path that is cide's
                // business rather than theirs.
                // The scheme is checked in Rust, not here — see `ext.openLink`. This only avoids
                // a round trip for the obviously-local cases.
                if (href.includes('://')) void extApi.openLink(href).catch(notifyFailure)
              }}
            />
          </div>
        )}
      </div>
    </div>
  )
}
