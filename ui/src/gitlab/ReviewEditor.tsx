import { cideHighlightStyle } from '@/editor/highlight'
import { useEffect, useRef, useState } from 'react'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
import { defaultKeymap } from '@codemirror/commands'
import {
  searchKeymap,
  highlightSelectionMatches,
  openSearchPanel,
} from '@codemirror/search'
import {
  foldGutter,
  foldKeymap,
  syntaxHighlighting,
} from '@codemirror/language'
import { loadLanguage } from '@/editor/languages'
import {
  polarityExtension,
  watchPolarity,
  polaritySlot,
} from '@/editor/cmPolarity'
import { diagnostics, file, gitlab, type Usage } from '@/ipc/client'
import {
  api,
  loadReview,
  documentKey,
  data,
  openDocument,
  prepareSource,
  roots,
  revealed,
  useGitLab,
  type Document,
} from './store'
import { message, reviewProject, visibleChanges, versionRefs } from './model'
import { ReviewDiff } from './ReviewDiff'
import { CommentWalk } from './CommentWalk'
import styles from './GitLab.module.css'

export function ReviewEditor({
  document: input,
  project,
  visible = true,
}: {
  document: Document
  project: string | null
  visible?: boolean
}) {
  const snapshot = useGitLab()
  const requested = snapshot.editor
  const doc =
    requested?.at &&
    JSON.stringify(documentKey(requested)) ===
      JSON.stringify(documentKey(input))
      ? { ...input, at: requested.at }
      : input
  const d = data.get(doc.review)
  const [loaded, setLoaded] = useState<{
    key: string
    text: { old: string; next: string }
  } | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const [root, setRoot] = useState(
    roots.get(`${doc.review}:${doc.refs.head_sha}`) ?? '',
  )
  const [navigation, setNavigation] = useState<
    { path: string; line: number; column: number }[]
  >([])
  const key = `${JSON.stringify(documentKey(doc))}:${doc.at?.line ?? 0}:${doc.at?.column ?? 0}`
  const text = loaded?.key === key ? loaded.text : null
  const [usages, setUsages] = useState<Usage[]>([])
  useEffect(() => {
    let current = true
    setLoaded(null)
    setRoot(roots.get(`${doc.review}:${doc.refs.head_sha}`) ?? '')
    setUsages([])
    setError('')
    async function load() {
      const d = await loadReview(doc.review)
      if (doc.mode === 'diff') return { old: '', next: '' }
      const change = doc.change
      const source = d.mr.source_project_id ?? d.mr.project_id
      if (doc.mode === 'source') {
        try {
          const directory = await prepareSource(doc.review, doc.refs.head_sha)
          if (current) setRoot(directory)
          const next = await api<string>({
            kind: 'sourceFile',
            review: doc.review,
            sha: doc.refs.head_sha,
            path: doc.path,
          })
          return { old: '', next }
        } catch (checkoutError) {
          if (current) {
            setRoot('')
            setError(
              `Source navigation unavailable: ${message(checkoutError)}. Showing the immutable GitLab file.`,
            )
          }
          const next = await api<string>({
            kind: 'file',
            review: doc.review,
            project: source,
            path: doc.path,
            sha: doc.refs.head_sha,
          })
          return { old: '', next }
        }
      }
      const old = change?.new_file
        ? ''
        : await api<string>({
            kind: 'file',
            review: doc.review,
            project: d.mr.target_project_id,
            path: change?.old_path ?? doc.oldPath ?? doc.path,
            sha: doc.refs.base_sha,
          })
      if (doc.mode === 'base') return { old: '', next: old }
      const next = change?.deleted_file
        ? ''
        : await api<string>({
            kind: 'file',
            review: doc.review,
            project: source,
            path: change?.new_path ?? doc.path,
            sha: doc.refs.head_sha,
          })
      return { old, next }
    }
    void load().then(
      (value) => {
        if (current) setLoaded({ key, text: value })
      },
      (e) => {
        if (current) setError(message(e))
      },
    )
    return () => {
      current = false
    }
  }, [key])
  useEffect(() => {
    if (!host.current || !text || doc.mode === 'diff') return
    let alive = true
    let cleanup = () => {}
    async function mount() {
      const language = await loadLanguage(doc.path)
      if (!alive || !host.current) return
      const polarity = polaritySlot()
      const common: Extension[] = [
        EditorState.readOnly.of(true),
        EditorView.editable.of(true),
        EditorView.contentAttributes.of({ 'aria-readonly': 'true' }),
        polarityExtension(polarity),
        keymap.of([...defaultKeymap, ...searchKeymap, ...foldKeymap]),
        highlightSelectionMatches(),
        syntaxHighlighting(cideHighlightStyle),
        foldGutter(),
        ...(language ? [language] : []),
      ]
      const side = (_name: 'old' | 'new'): Extension[] => [
        ...common,
        lineNumbers(),
        keymap.of([
          {
            key: 'Shift-F12',
            run: (v) => {
              void references(v)
              return true
            },
          },
          {
            key: 'F12',
            run: (v) => {
              void definition(v)
              return true
            },
          },
        ]),
      ]
      const single = new EditorView({
        parent: host.current,
        state: EditorState.create({ doc: text!.next, extensions: side('new') }),
      })
      view.current = single
      if (doc.at && view.current) {
        const line = view.current.state.doc.line(
          Math.min(doc.at.line, view.current.state.doc.lines),
        )
        const anchor = Math.min(line.to, line.from + doc.at.column - 1)
        view.current.dispatch({
          selection: { anchor },
          effects: EditorView.scrollIntoView(anchor, { y: 'center' }),
        })
      }
      const stops = [single].map((v) => watchPolarity(v, polarity))
      if (doc.mode === 'source' && root)
        void diagnostics.didOpen(
          reviewProject(doc.review, doc.refs.head_sha),
          `${root}/${doc.path}`,
          1,
          text!.next,
        )
      cleanup = () => {
        stops.forEach((stop) => stop())
        single?.destroy()
        view.current = null
        if (doc.mode === 'source' && root)
          void diagnostics.didClose(
            reviewProject(doc.review, doc.refs.head_sha),
            `${root}/${doc.path}`,
          )
      }
    }
    async function references(v: EditorView) {
      setError('')
      try {
        if (doc.mode !== 'source')
          throw new Error('Open the current MR source file to find references.')
        const directory =
          root || (await prepareSource(doc.review, doc.refs.head_sha))
        const head = v.state.selection.main.head,
          line = v.state.doc.lineAt(head)
        const answer = await diagnostics.usages(
          reviewProject(doc.review, doc.refs.head_sha),
          `${directory}/${doc.path}`,
          line.number,
          head - line.from + 1,
        )
        if (!alive) return
        if (answer.kind === 'found') {
          setUsages(
            answer.rows.filter((row) => row.path.startsWith(directory + '/')),
          )
          if (answer.truncated)
            setError('Reference results were truncated by the language server.')
          else if (!answer.rows.length) setError('No references found.')
        } else
          setError(
            answer.kind === 'unavailable'
              ? answer.reason
              : 'No symbol at this position.',
          )
      } catch (e) {
        if (alive) setError(message(e))
      }
    }
    async function definition(v: EditorView) {
      setError('')
      try {
        if (doc.mode !== 'source')
          throw new Error(
            'Open MR source file to navigate definitions in its checked-out source tree.',
          )
        const directory =
          root || (await prepareSource(doc.review, doc.refs.head_sha))
        const caret = v.state.selection.main.head
        const line = v.state.doc.lineAt(caret)
        const answer = await diagnostics.definition(
          reviewProject(doc.review, doc.refs.head_sha),
          `${directory}/${doc.path}`,
          line.number,
          caret - line.from + 1,
        )
        if (!alive) return
        if (answer.kind === 'found') {
          if (!answer.path.startsWith(directory + '/'))
            throw new Error('This definition is outside the MR source tree.')
          setNavigation((old) => [
            ...old,
            {
              path: doc.path,
              line: line.number,
              column: caret - line.from + 1,
            },
          ])
          openDocument({
            review: doc.review,
            refs: doc.refs,
            path: answer.path.slice(directory.length + 1),
            mode: 'source',
            at: { line: answer.line, column: answer.column },
          })
        } else
          setError(
            answer.kind === 'unavailable'
              ? answer.reason
              : 'No definition at this position.',
          )
      } catch (e) {
        setError(message(e))
      }
    }
    void mount().catch((e) => setError(message(e)))
    return () => {
      alive = false
      cleanup()
    }
  }, [text, key, root])
  async function local() {
    if (!project || !d) return
    setBusy(true)
    setError('')
    try {
      const sourceUrl = d.mr.source_project?.web_url
      if (!sourceUrl)
        throw new Error('GitLab source project information is unavailable.')
      const path = await gitlab.localFile(
        doc.review,
        project,
        sourceUrl,
        doc.path,
      )
      if (!path)
        throw new Error(
          'The MR source project does not match a repository in the current project.',
        )
      await file.open(project, path)
    } catch (e) {
      setError(message(e))
    } finally {
      setBusy(false)
    }
  }
  const visibleFiles = d
    ? visibleChanges(
        d.version.diffs ?? [],
        snapshot.board.preferences.excludedFiles,
        snapshot.board.preferences.excludeEnabled && !revealed.has(doc.review),
      )
    : []
  const index = visibleFiles.findIndex((c) => c.new_path === doc.path)
  function nextFile(offset: number) {
    const change = visibleFiles[index + offset]
    if (change)
      openDocument({
        review: doc.review,
        path: change.new_path,
        change,
        mode: 'diff',
        refs: d ? versionRefs(d.version) : doc.refs,
      })
  }
  return (
    <section className={styles.editor} aria-label="MR editor">
      <div className={styles.bar}>
        <strong>
          !{d?.mr.iid} · {doc.path}
        </strong>
        <span className={styles.muted}>
          {doc.mode === 'diff'
            ? `Base ${doc.refs.base_sha.slice(0, 8)} → MR ${doc.refs.head_sha.slice(0, 8)}`
            : doc.mode === 'base'
              ? doc.refs.base_sha.slice(0, 8)
              : doc.refs.head_sha.slice(0, 8)}{' '}
          · {doc.mode}
        </span>
      </div>
      <div className={styles.bar}>
        <button
          title="Previous MR file (Alt+PageUp)"
          disabled={index <= 0}
          onClick={() => nextFile(-1)}
        >
          Previous file
        </button>
        <button
          title="Next MR file (Alt+PageDown)"
          disabled={index < 0 || index >= visibleFiles.length - 1}
          onClick={() => nextFile(1)}
        >
          Next file
        </button>
        {doc.mode !== 'diff' && (
          <button
            onClick={() => {
              if (view.current) openSearchPanel(view.current)
            }}
          >
            Find
          </button>
        )}
        <button
          disabled={doc.change?.deleted_file || busy}
          onClick={() => openDocument({ ...doc, mode: 'source' })}
        >
          Open MR source file
        </button>
        <button
          disabled={doc.change?.new_file}
          onClick={() => openDocument({ ...doc, mode: 'base' })}
        >
          Open base file
        </button>
        {doc.mode === 'source' && !root && (
          <button
            disabled={busy}
            onClick={() => {
              setBusy(true)
              void prepareSource(doc.review, doc.refs.head_sha)
                .then(
                  (directory) => {
                    setRoot(directory)
                    setError('')
                  },
                  (e) => setError(message(e)),
                )
                .finally(() => setBusy(false))
            }}
          >
            Retry source navigation
          </button>
        )}
        {project && (
          <button disabled={busy} onClick={() => void local()}>
            Open local working-tree file
          </button>
        )}
        {/* The panel's section buttons already open Discussions; the diff header carries
            the walk through this file's comments instead (Alt+Shift+Down/Up). */}
        {doc.mode === 'diff' && <CommentWalk document={doc} />}
        {navigation.length > 0 && (
          <button
            onClick={() => {
              const previous = navigation.at(-1)
              if (previous) {
                setNavigation((n) => n.slice(0, -1))
                openDocument({
                  review: doc.review,
                  refs: doc.refs,
                  path: previous.path,
                  mode: 'source',
                  at: { line: previous.line, column: previous.column },
                })
              }
            }}
          >
            Back
          </button>
        )}
      </div>
      {error && (
        <div role="alert" className={styles.error}>
          {error}
        </div>
      )}
      {!text && !error && <p>Loading immutable source…</p>}
      <div className={styles.body}>
        {doc.mode === 'diff' ? (
          <ReviewDiff document={doc} visible={visible} />
        ) : (
          <div ref={host} className={styles.surface} />
        )}
        {usages.length > 0 && (
          <aside className={styles.inline}>
            {usages.length > 0 && (
              <div>
                <strong>References</strong>
                {usages.map((u) => (
                  <button
                    className={styles.file}
                    key={`${u.path}:${u.line}:${u.column}`}
                    onClick={() =>
                      openDocument({
                        review: doc.review,
                        refs: doc.refs,
                        path: u.path.slice(root.length + 1),
                        mode: 'source',
                        at: { line: u.line, column: u.column },
                      })
                    }
                  >
                    {u.rel}:{u.line} {u.text}
                  </button>
                ))}
              </div>
            )}
            {doc.mode === 'source' && (
              <p className={styles.muted}>
                Read-only review source · F12 goes to definition; Shift+F12
                finds references.
              </p>
            )}
          </aside>
        )}
      </div>
    </section>
  )
}
