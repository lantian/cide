/**
 * A diff that Claude Code is blocked on, wired to the broker that is holding its turn open.
 *
 * `DiffPane` itself is deliberately IPC-free — it takes documents and callbacks — so this is
 * the piece that fetches the documents and turns a button press into an answer. Keeping the
 * split there means the diff view can be rendered from a fixture in a test without a running
 * server, and this file stays small enough to read in one go.
 *
 * # The rule this component exists to honour
 *
 * The model is sitting still until one of these three callbacks fires. There is no timeout
 * on the other side: an unanswered diff is a conversation that never resumes, with nothing on
 * screen to explain it. So every path out of this component ends in an answer — including
 * the failure path, where the contents cannot be fetched and the honest thing is to reject
 * rather than to show an error and leave the agent waiting behind it.
 */
import { useEffect, useState } from 'react'
import { DiffPane } from './DiffPane'
import { claude } from '@/ipc/client'
import type { DiffAnswer } from '@/ipc/client'

export interface ClaudeDiffPaneProps {
  project: string
  requestId: string
  oldPath: string
  newPath: string
}

type Load =
  | { state: 'loading' }
  | { state: 'ready'; original: string; proposed: string }
  | { state: 'gone'; reason: string }

export function ClaudeDiffPane({ project, requestId, oldPath, newPath }: ClaudeDiffPaneProps) {
  const [load, setLoad] = useState<Load>({ state: 'loading' })

  useEffect(() => {
    let disposed = false
    setLoad({ state: 'loading' })

    claude
      .diffContent(project, requestId)
      .then((c) => {
        if (!disposed) setLoad({ state: 'ready', original: c.original, proposed: c.proposed })
      })
      .catch((e: unknown) => {
        // The request is already gone — answered from another window, or cancelled when
        // something else closed. Not an error worth a dialog: the tab is stale and says so.
        if (!disposed) setLoad({ state: 'gone', reason: String(e) })
      })

    return () => {
      disposed = true
    }
    // Keyed on the request, not on the paths: two diffs can name the same file, and
    // rebuilding on a path change would discard a half-made edit.
  }, [project, requestId])

  const answer = (outcome: DiffAnswer) => {
    void claude.answer(project, requestId, outcome)
  }

  if (load.state === 'loading') {
    return <div className="diffPending">Loading the proposed change…</div>
  }

  if (load.state === 'gone') {
    // Deliberately not offering the three buttons here. There is nothing left to answer —
    // the broker no longer holds this request — and presenting controls that silently do
    // nothing would be worse than saying so.
    return (
      <div className="diffPending">
        This diff is no longer waiting for an answer. It was resolved or cancelled elsewhere.
      </div>
    )
  }

  return (
    <DiffPane
      requestId={requestId}
      oldPath={oldPath}
      newPath={newPath}
      original={load.original}
      proposed={load.proposed}
      onAccept={(finalContents) => answer({ kind: 'acceptedEdited', contents: finalContents })}
      onAcceptAsProposed={() => answer({ kind: 'acceptedAsIs' })}
      onReject={() => answer({ kind: 'rejected' })}
    />
  )
}
