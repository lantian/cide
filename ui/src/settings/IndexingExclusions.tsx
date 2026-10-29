import { useEffect, useRef, useState } from 'react'
import { settings as api, type ExplorerSettings, type ProjectId } from '@/ipc/client'
import { Field, Textarea } from '@/kit/components/Field'
import { fsMessage } from '@/sidebar/fsError'
import { ActionButton, Group, InlineControls, Row, Segmented } from './controls'
import styles from './IndexingExclusions.module.css'

export function IndexingExclusions({ project, explorer }: { project: ProjectId | null; explorer: ExplorerSettings }) {
  const [chosenScope, setScope] = useState<'global' | 'project'>('global')
  const scope = project === null ? 'global' : chosenScope
  const [reload, setReload] = useState(0)
  const [ready, setReady] = useState(true)
  const [override, setOverride] = useState<string[] | null>(null)
  const [draft, setDraft] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const epoch = useRef(0)
  const globalText = explorer.indexExcludedFolders.join('\n')
  useEffect(() => {
    const attempt = ++epoch.current
    setError(null)
    setSaving(false)
    if (scope === 'global' || project === null) {
      setDraft(globalText)
      setReady(true)
      setLoading(false)
      return
    }
    setLoading(true)
    setReady(false)
    void api.projectIndexing(project).then((value) => {
      if (epoch.current !== attempt) return
      setOverride(value)
      setReady(true)
      setDraft(value === null ? globalText : value.join('\n'))
    }, (reason: unknown) => {
      if (epoch.current === attempt) setError(fsMessage(reason))
    }).finally(() => { if (epoch.current === attempt) setLoading(false) })
    return () => { epoch.current++ }
  }, [project, scope, globalText, reload])

  async function save(inherit = false) {
    if (saving || loading || !ready) return
    const attempt = epoch.current
    setSaving(true)
    setError(null)
    const rules = draft.split('\n').map((line) => line.trim()).filter(Boolean)
    try {
      if (scope === 'project' && project !== null) {
        const value = await api.setProjectIndexing(project, inherit ? null : rules)
        if (epoch.current === attempt) {
          setOverride(value)
          setDraft(value === null ? globalText : value.join('\n'))
        }
      } else {
        const latest = await api.get()
        await api.set({ explorer: { ...latest.explorer, indexExcludedFolders: rules } })
      }
    } catch (reason) {
      if (epoch.current === attempt) setError(fsMessage(reason))
    } finally {
      if (epoch.current === attempt) setSaving(false)
    }
  }

  return <Group title="Indexing exclusions">
    <Row label="Apply exclusions to" anchor="Indexing exclusions" hint="Project overrides are saved locally on this machine."
      control={<Segmented label="Indexing exclusions scope" value={scope} options={project === null
        ? [{ value: 'global', label: 'Global' }]
        : [{ value: 'global', label: 'Global' }, { value: 'project', label: 'This project' }]}
        onChange={setScope} />} />
    <div className={styles.editor}>
      <p>One root-relative path or glob per line, such as <code>.cide/tasks</code> or <code>**/node_modules</code>. Applies to every project root. Folders remain browsable; their contents are excluded from file, content and symbol search, even with Show ignored files enabled.</p>
      {scope === 'project' && <p>{override === null ? 'Using global exclusions. Saving creates a project override.' : 'This list replaces the global exclusions.'}</p>}
      <Field label="Folders excluded from indexing" error={error}>
        {({ id, describedBy, invalid }) => <Textarea id={id} aria-describedby={describedBy} aria-invalid={invalid}
          aria-label="Folders excluded from indexing" rows={5} value={draft} disabled={loading || saving || !ready}
          onChange={(e) => setDraft(e.target.value)} spellCheck={false} />}
      </Field>
      <InlineControls>
        {!ready && !loading && <ActionButton label="Retry loading exclusions" onClick={() => setReload((value) => value + 1)} />}
        <ActionButton disabled={loading || saving || !ready} onClick={() => { void save() }} label={saving ? 'Saving…' : 'Save exclusions'} />
        {scope === 'project' && <ActionButton disabled={loading || saving || !ready || override === null} onClick={() => { void save(true) }} label="Use global exclusions" />}
      </InlineControls>
      <p>An empty list excludes no folders. Saving rebuilds indexes whose effective exclusions changed.</p>
    </div>
  </Group>
}
