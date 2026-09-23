import { CommentBox, Thread } from './Discussions'
import { wholeFileSegments } from '@/panes/diffRows'
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react'
import { GitDiffView, useDiffTokens } from '@/panes/GitDiffPane'
import {
  getDiffView,
  getServerDiffView,
  setDiffView,
  subscribeDiffView,
} from '@/editor/diffViewMode'
import type { RevisionDiff } from '@/ipc/client'
import { api, data, documentKey, useGitLab, type Document } from './store'
import { linePosition, message } from './model'
import type { Position } from './types'
import styles from './GitLab.module.css'

/** Only remote fetching and review actions live here; drawing stays in Cide's diff viewer. */
export function ReviewDiff({
  document: doc,
  visible,
}: {
  document: Document
  visible: boolean
}) {
  const snapshot = useGitLab()
  const [position, onPosition] = useState<Position | null>(null)
  const review = data.get(doc.review)
  const [diff, setDiff] = useState<RevisionDiff | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set())
  const [gaps, setGaps] = useState<ReadonlySet<number>>(new Set())
  const [current, setCurrent] = useState(0)
  const [line, setLine] = useState<{ side: 'old' | 'new'; line: number }>()
  const host = useRef<HTMLDivElement>(null)
  const mode = useSyncExternalStore(
    subscribeDiffView,
    getDiffView,
    getServerDiffView,
  )
  const tokens = useDiffTokens(diff)
  const key = JSON.stringify(documentKey(doc))
  const requested = snapshot.editor
  const at =
    requested && JSON.stringify(documentKey(requested)) === key
      ? requested.at
      : doc.at
  const threads = useMemo(() => {
    const map = new Map<string, NonNullable<typeof review>['discussions']>()
    for (const thread of review?.discussions ?? []) {
      const p = thread.notes.find((n) => n.position)?.position
      if (
        !p ||
        p.head_sha !== doc.refs.head_sha ||
        p.base_sha !== doc.refs.base_sha ||
        (p.new_path !== doc.path && p.old_path !== doc.path)
      )
        continue
      const anchor =
        p.new_line == null ? `old:${p.old_line}` : `new:${p.new_line}`
      map.set(anchor, [...(map.get(anchor) ?? []), thread])
    }
    return map
  }, [review?.discussions, key])
  useEffect(() => {
    if (!diff) return
    const segments = wholeFileSegments(diff.hunks, diff.newText)
    const needed =
      segments?.filter(
        (segment) =>
          segment.kind === 'gap' &&
          segment.lines.some(
            (row) =>
              threads.has(`old:${row.oldLineno}`) ||
              threads.has(`new:${row.newLineno}`),
          ),
      ) ?? []
    if (needed.length)
      setGaps(
        (old) =>
          new Set([
            ...old,
            ...needed.flatMap((segment) =>
              segment.kind === 'gap' ? [segment.index] : [],
            ),
          ]),
      )
  }, [diff, threads])
  useEffect(() => {
    let alive = true
    setDiff(null)
    onPosition(null)
    setError(null)
    void api<RevisionDiff>({
      kind: 'comparison',
      document: documentKey(doc),
    }).then(
      (value) => {
        if (alive) {
          setDiff(value)
          setCurrent(0)
        }
      },
      (error) => {
        if (alive) setError(message(error))
      },
    )
    return () => {
      alive = false
    }
  }, [key])
  useEffect(() => {
    if (!at || !diff || !visible) return
    const side = at.side ?? 'new'
    setLine({ side, line: at.line })
    // A discussion may be attached to context hidden inside a collapsed gap.
    setGaps(
      new Set(Array.from({ length: diff.hunks.length * 2 + 2 }, (_, i) => i)),
    )
    const frame = requestAnimationFrame(() => {
      host.current
        ?.querySelector(
          `[data-review-side="${side}"][data-review-line="${at.line}"]`,
        )
        ?.scrollIntoView({ block: 'center' })
    })
    return () => cancelAnimationFrame(frame)
  }, [at, diff, visible])
  function select(side: 'old' | 'new', number: number) {
    setLine({ side, line: number })
    if (
      data.get(doc.review)?.version.head_commit_sha !== doc.refs.head_sha ||
      data.get(doc.review)?.version.base_commit_sha !== doc.refs.base_sha
    ) {
      onPosition(null)
      return
    }
    const change = doc.change
    if (change?.diff) {
      onPosition(linePosition(change, doc.refs, side, number))
      return
    }
    const row = diff?.hunks
      .flatMap((h) => h.lines)
      .find((l) => (side === 'old' ? l.oldLineno : l.newLineno) === number)
    onPosition(
      row
        ? {
            ...doc.refs,
            position_type: 'text',
            old_path: doc.oldPath ?? doc.path,
            new_path: doc.path,
            ...(row.oldLineno === null ? {} : { old_line: row.oldLineno }),
            ...(row.newLineno === null ? {} : { new_line: row.newLineno }),
          }
        : null,
    )
  }
  return (
    <div ref={host} className={styles.surface}>
      <GitDiffView
        readOnly
        diff={diff}
        path={doc.path}
        busy={!diff && !error}
        reason={error}
        note={null}
        collapsed={collapsed}
        revisions={{
          from: doc.refs.base_sha.slice(0, 8),
          to: doc.refs.head_sha.slice(0, 8),
        }}
        view={mode.view}
        onView={mode.writable ? setDiffView : undefined}
        tokens={tokens}
        visible={visible}
        currentChange={current}
        onCurrentChange={setCurrent}
        onCollapse={(h) =>
          setCollapsed((old) => {
            const next = new Set(old)
            if (next.has(h)) next.delete(h)
            else next.add(h)
            return next
          })
        }
        expandedGaps={gaps}
        onExpandGap={(gap) => setGaps((old) => new Set([...old, gap]))}
        onReviewLine={select}
        reviewLine={line}
        renderAfterLine={(oldLine, newLine, side) => {
          const keys = [
            ...(side !== 'new' ? [`old:${oldLine}`] : []),
            ...(side !== 'old' ? [`new:${newLine}`] : []),
          ]
          const anchored = keys.flatMap((key) => threads.get(key) ?? [])
          const compose =
            position && line && keys.includes(`${line.side}:${line.line}`)
          if (!anchored.length && !compose) return null
          return (
            <div className={styles.inlineThreads}>
              {anchored.map((thread) => (
                <Thread
                  key={thread.id}
                  review={doc.review}
                  thread={thread}
                  inline
                />
              ))}
              {compose && (
                <div className={styles.inlineComposer}>
                  <header className={styles.threadHeader}>
                    <strong>
                      New thread · {line.side} line {line.line}
                    </strong>
                    <button
                      aria-label="Cancel inline comment"
                      onClick={() => onPosition(null)}
                    >
                      ×
                    </button>
                  </header>
                  <CommentBox
                    key={JSON.stringify(position)}
                    review={doc.review}
                    position={position}
                    label="Start thread"
                    onSent={() => onPosition(null)}
                  />
                </div>
              )}
            </div>
          )
        }}
      />
    </div>
  )
}
