import { useEffect, useRef, useState } from 'react'
import { Compartment, EditorState } from '@codemirror/state'
import { EditorView, lineNumbers } from '@codemirror/view'
import { syntaxHighlighting } from '@codemirror/language'
import { file } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { loadLanguage } from '@/editor/languages'
import { cideHighlightStyle } from '@/editor/highlight'
import { polarityExtension, polaritySlot, watchPolarity } from '@/editor/cmPolarity'
import { EditorSurface } from '@/editor/EditorSurface'
import { openDoc, closeDoc } from '@/editor/docSync'
import { markSource, type SourceMark } from '@/editor/sourceMark'
import { location, type Result } from './model'
import styles from './Search.module.css'

/** Modal previews stay passive; pinned readers opt into the normal read-only editor. */
export function SourcePreview({ result, project, editorId }: { result: Result | null; project: string | null; editorId?: string | undefined }) {
  const at = result === null ? null : location(result)
  const path = at?.path ?? null
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const marks = useRef(new Compartment())
  const [loaded, setLoaded] = useState<{ path: string; text: string } | null>(null)
  const [failure, setFailure] = useState<{ path: string; message: string } | null>(null)
  const [version, setVersion] = useState(0)
  useEffect(() => {
    if (path === null) return
    let alive = true
    let stopTheme = () => {}
    setLoaded(null); setFailure(null)
    void file.read(path).then(async doc => {
      if (!alive) return
      if (editorId) { setLoaded({ path, text: doc.text }); return }
      const language = doc.text.length <= 2 * 1024 * 1024 ? await loadLanguage(path) : null
      if (!alive || !host.current) return
      const polarity = polaritySlot()
      const editor = new EditorView({
        parent: host.current,
        state: EditorState.create({ doc: doc.text, extensions: [
          EditorState.readOnly.of(true), EditorView.editable.of(false),
          EditorView.contentAttributes.of({ tabindex: '0', 'aria-label': 'Source preview', 'aria-readonly': 'true' }),
          lineNumbers(), syntaxHighlighting(cideHighlightStyle), polarityExtension(polarity),
          marks.current.of([]), ...(language ? [language] : []),
          EditorView.theme({
            '&': { height: '100%', backgroundColor: 'var(--bg)', color: 'var(--text)' },
            '.cm-scroller': { overflow: 'auto', fontFamily: 'var(--font-mono)', fontSize: 'var(--fs-ui-12)' },
            '.cm-gutters': { backgroundColor: 'var(--panel)', color: 'var(--dim)', borderColor: 'var(--border-soft)' },
            '.cm-content': { padding: '8px 0' },
          }),
        ] }),
      })
      view.current = editor
      stopTheme = watchPolarity(editor, polarity)
      setLoaded({ path, text: doc.text })
      setVersion(n => n + 1)
    }).catch(error => {
      const detail = errorText(error)
      if (alive) setFailure({ path, message: detail && detail !== '[object Object]' && detail !== 'undefined' && detail !== 'null'
        ? detail : 'The file could not be read. It may have moved or become unavailable.' })
    })
    return () => { alive = false; stopTheme(); view.current?.destroy(); view.current = null }
  }, [path, editorId])
  useEffect(() => {
    if (view.current) markSource(view.current, marks.current, at)
  }, [at?.line, at?.column, at?.endColumn, path, version])
  const ready = path !== null && loaded?.path === path
  const error = failure?.path === path ? failure?.message : null
  return (
    <section className={styles.preview} aria-label="Result preview" data-audit="searchPreview">
      <div className={styles.previewPath} title={path ?? undefined}>{path ?? (result?.kind === 'action' ? result.command.group : 'Source preview')}</div>
      {result?.kind === 'action' ? <div className={styles.actionDetail}>
        <strong>{result.command.title}</strong>
        {result.shortcut && <p>{result.shortcut}</p>}
        <p>{result.command.unavailable ?? 'Press Enter to run this action.'}</p>
      </div> : !ready ? <div className={styles.previewStatus} role="status">
        <div>{path === null ? 'Select a result to preview its source.' : error ? <><strong>Preview unavailable</strong><p>{error}</p></> : 'Loading source…'}</div>
      </div> : null}
      {editorId ? ready && at && loaded && <div className={styles.source}>
        <PinnedSourceEditor key={path} path={path} project={project} text={loaded.text} id={editorId} at={at} />
      </div> : <div ref={host} className={styles.source} hidden={!ready} />}
    </section>
  )
}

function PinnedSourceEditor({ path, project, text, id, at }: { path: string; project: string | null; text: string; id: string; at: SourceMark }) {
  useEffect(() => {
    if (!project) return
    // Reference counting protects an already-open (possibly unsaved) tab's LSP document.
    openDoc(project, path, text)
    return () => closeDoc(path)
  }, [project, path, text])
  return <EditorSurface path={path} project={project ?? undefined} doc={text} readOnly
    embedded={{ revealKey: `search-preview:${id}:${path}` }} markedRange={at} />
}
