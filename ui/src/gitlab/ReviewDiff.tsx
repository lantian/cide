import { CommentBox, Thread } from './Discussions'
import { DraftCard, draftAnchor, draftOn } from './Drafts'
import { wholeFileSegments, type FileSegment } from '@/panes/diffRows'
import {
  claimCommentNav,
  orderStops,
  publishCommentCursor,
  stepStop,
  type CommentStop,
} from './commentNav'
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
  // Local drafts on this exact diff, keyed like `threads`. (M85) A draft written against an
  // older MR version is not drawn here — its line may mean something else now — and stays in
  // the Drafts list marked outdated.
  const pending = useMemo(() => {
    const map = new Map<string, NonNullable<typeof review>['drafts']>()
    for (const draft of review?.drafts ?? []) {
      const anchor = draftAnchor(draft)
      if (
        !anchor ||
        !draftOn(draft, doc.path, doc.refs.base_sha, doc.refs.head_sha)
      )
        continue
      map.set(anchor, [...(map.get(anchor) ?? []), draft])
    }
    return map
  }, [review?.drafts, key])
  const navKey = useMemo(
    () => ({
      review: doc.review,
      path: doc.path,
      baseSha: doc.refs.base_sha,
      headSha: doc.refs.head_sha,
    }),
    [key],
  )
  const stops = useMemo(
    () => orderStops(new Set([...threads.keys(), ...pending.keys()])),
    [threads, pending],
  )
  const stopsRef = useRef(stops)
  stopsRef.current = stops
  const cursor = useRef(-1)
  const [focusStop, setFocusStop] = useState<CommentStop | null>(null)
  // Stops move when a draft lands or a thread is resolved elsewhere; keep the cursor on the
  // same line if it still carries a comment, else restart the walk rather than point past the end.
  useEffect(() => {
    const at = focusStop
      ? stops.findIndex((s) => s.side === focusStop.side && s.line === focusStop.line)
      : -1
    cursor.current = at
    publishCommentCursor(navKey, at, stops.length)
  }, [stops, navKey])
  useEffect(
    () =>
      claimCommentNav(navKey, {
        step(delta) {
          const all = stopsRef.current
          const next = stepStop(all.length, cursor.current, delta)
          if (next === null) return false
          const stop = all[next]!
          cursor.current = next
          publishCommentCursor(navKey, next, all.length)
          setLine({ side: stop.side, line: stop.line })
          setFocusStop({ ...stop })
          return true
        },
      }),
    [navKey],
  )
  // After the stop's gap is open and the row painted: centre it and move focus into the
  // comment block, so Tab reaches its Reply/Discuss buttons without the mouse.
  useEffect(() => {
    if (!focusStop || !diff || !visible) return
    const segments = wholeFileSegments(diff.hunks, diff.newText)
    const gap = gapHolding(segments, focusStop.side, focusStop.line)
    if (gap !== null && !gaps.has(gap)) {
      setGaps((old) => new Set([...old, gap]))
      return // re-runs with the gap open
    }
    const frame = requestAnimationFrame(() => {
      const block = host.current?.querySelector<HTMLElement>(
        `[data-comment-anchor="${focusStop.side}:${focusStop.line}"]`,
      )
      const row = host.current?.querySelector(
        `[data-review-side="${focusStop.side}"][data-review-line="${focusStop.line}"]`,
      )
      ;(row ?? block)?.scrollIntoView({ block: 'center' })
      block?.focus({ preventScroll: true })
    })
    return () => cancelAnimationFrame(frame)
  }, [focusStop, diff, visible, gaps])
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
              threads.has(`new:${row.newLineno}`) ||
              pending.has(`old:${row.oldLineno}`) ||
              pending.has(`new:${row.newLineno}`),
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
  }, [diff, threads, pending])
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
    // A discussion may be attached to context hidden inside a folded gap. Open that one gap
    // only: opening every gap (as this did while the review drew whole files anyway) would
    // undo the hunks-only view the moment anything jumped to a line.
    const gap = gapHolding(
      wholeFileSegments(diff.hunks, diff.newText),
      side,
      at.line,
    )
    if (gap !== null) setGaps((old) => new Set([...old, gap]))
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
        foldUnchanged
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
          const drafted = keys.flatMap((key) => pending.get(key) ?? [])
          const compose =
            position && line && keys.includes(`${line.side}:${line.line}`)
          if (!anchored.length && !drafted.length && !compose) return null
          // The comment walk's landing: the first anchor of this row that holds a comment.
          const anchor = keys.find((k) => threads.has(k) || pending.has(k))
          return (
            <div
              className={styles.inlineThreads}
              data-comment-anchor={anchor}
              tabIndex={anchor === undefined ? undefined : -1}
            >
              {drafted.map((draft) => (
                <DraftCard
                  key={draft.id}
                  review={doc.review}
                  draft={draft}
                  inline
                />
              ))}
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

/** The index of the unchanged gap that holds this line, or `null` when it is in a hunk. */
function gapHolding(
  segments: readonly FileSegment[] | null,
  side: 'old' | 'new',
  line: number,
): number | null {
  for (const segment of segments ?? []) {
    if (segment.kind !== 'gap') continue
    if (
      segment.lines.some(
        (row) => (side === 'old' ? row.oldLineno : row.newLineno) === line,
      )
    )
      return segment.index
  }
  return null
}
