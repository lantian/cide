/**
 * A read-only `docker inspect` document. (M43)
 *
 * # Why an editor surface and not a table of fields
 *
 * Because `inspect` is the one place where *everything* is worth showing, and cide cannot know
 * in advance which field somebody is looking for. A modelled view would be a second, lossy copy
 * of a two-hundred-field document Docker extends every release — and the field a user is hunting
 * is disproportionately likely to be one of the new ones. The panel draws rows for the handful
 * that matter; this is the whole truth behind them.
 *
 * `EditorSurface` with `readOnly` is `RevisionPane`'s reuse, for its reason: search, folding,
 * syntax colouring, the status bar and the find bar are all already correct here, and a bespoke
 * `<pre>` would have none of them.
 *
 * # Why the document is re-read on every mount and never stored
 *
 * `TabKind::Docker` carries a *target*, not text. A container's inspect document is a claim
 * about its state at the moment it was read, and a tab restored from `workspace.json` would
 * assert yesterday's. `tab_outlives_close` answers `false` for this variant for the same reason.
 */
import { useEffect, useState } from 'react'

import { EditorSurface } from '@/editor/EditorSurface'
import { docker as dockerApi, type InspectTarget } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'

import styles from './DockerInspectPane.module.css'

export interface DockerInspectPaneProps {
  readonly target: InspectTarget
  /** The stored tab title — `shop-db-1 : container`. */
  readonly title: string
}

/**
 * The identity this buffer registers under.
 *
 * A `docker://` path, and it is deliberately **not** a real one. `EditorSurface`'s `identity`
 * exists so a document that is not the working file cannot be mistaken for it — the same reason
 * `RevisionPane` passes one — and a made-up scheme is what guarantees no `file_write` can ever
 * resolve it. The `.json` suffix is doing real work: it is what picks the language.
 */
function identityFor(target: InspectTarget): string {
  switch (target.kind) {
    case 'container':
      return `docker://container/${target.id}.json`
    case 'image':
      return `docker://image/${target.id}.json`
    case 'volume':
      return `docker://volume/${target.name}.json`
    case 'network':
      return `docker://network/${target.id}.json`
  }
}

export function DockerInspectPane({ target, title }: DockerInspectPaneProps) {
  const [doc, setDoc] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  useEffect(() => {
    let disposed = false
    setDoc(null)
    setFailure(null)
    dockerApi
      .inspect(target)
      .then((text) => {
        if (!disposed) setDoc(text)
      })
      .catch((error: unknown) => {
        // The whole point of showing this rather than an empty buffer: a container that has been
        // removed since the row was drawn answers "No such container", and an empty document
        // would read as a container with nothing in it.
        if (!disposed) setFailure(errorText(error))
      })
    return () => {
      disposed = true
    }
    // `target` is a fresh object identity on every render of the parent, so the *fields* are the
    // dependency — a `[target]` here would re-read the document on every keystroke anywhere.
  }, [target])

  if (failure !== null) {
    return <div className={styles.notice}>{failure}</div>
  }
  if (doc === null) {
    // Deliberately blank rather than a spinner: the read is one round trip to a local socket, and
    // a spinner that flashes for 20ms is noise. A failure replaces this within the same frame.
    return <div className={styles.pane} />
  }

  return (
    <div className={styles.pane}>
      <EditorSurface
        path={`${title}.json`}
        identity={identityFor(target)}
        doc={doc}
        readOnly
        // No diagnostics exist for a daemon's own document, and `none` removes the lint gutter
        // rather than leaving an empty column — `RevisionPane`'s note, and its reason.
        highlight="none"
      />
    </div>
  )
}
