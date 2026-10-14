/** The remembered-project picker is also mounted in the empty shell and detached windows. */
import { useEffect, useMemo, useRef, useState } from 'react'
import { projectMenu, type RecentEntry } from '@/ipc/client'
import { filterProjects, MISSING_REASON } from '@/chrome/menuModel'
import { browseForProject } from '@/chrome/projectOpen'
import { Button } from '@/kit/components/Button'
import { PickerRow, PickerStatus } from '@/kit/components/Overlay'
import { Icon } from '@/icons/Icon'
import { ModalShell, Hint } from './ModalShell'
import { isListKey, listAction } from './listKeys'
import styles from './ProjectPicker.module.css'

export function ProjectPicker({ onDismiss }: { onDismiss: () => void }) {
  const [query, setQuery] = useState('')
  const [projects, setProjects] = useState<readonly RecentEntry[] | null>(null)
  const [failed, setFailed] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const rowRefs = useRef(new Map<string, HTMLDivElement>())

  useEffect(() => {
    let live = true
    setProjects(null)
    setFailed(false)
    void projectMenu.recent().then(
      (entries) => { if (live) setProjects(entries) },
      () => { if (live) setFailed(true) },
    )
    return () => { live = false }
  }, [attempt])

  const matches = useMemo(() => filterProjects(projects ?? [], query), [projects, query])
  const available = useMemo(() => matches.filter((entry) => entry.exists), [matches])
  const selectedIndex = Math.max(0, available.findIndex((entry) => entry.project.path === selectedPath))
  const selected = available[selectedIndex]?.project.path ?? null

  useEffect(() => {
    if (selected !== null) rowRefs.current.get(selected)?.scrollIntoView({ block: 'nearest' })
  }, [selected])

  const open = (entry: RecentEntry | undefined) => {
    if (entry === undefined || !entry.exists) return
    onDismiss()
    // Recheck existence in Rust and use the usual opening path (including saved layouts).
    // A directory disappearing after the list loaded is reported by the Failures surface.
    void projectMenu.reopen(entry.project.path)
  }

  return (
    <div className={styles.host} onKeyDown={(ev) => {
      if (ev.key === 'Escape' && !ev.nativeEvent.isComposing) {
        ev.preventDefault()
        ev.stopPropagation()
        onDismiss()
      }
    }}>
      <ModalShell
        label="Open project"
        prompt={<Icon name="search" size={1} />}
        value={query}
        placeholder="Filter projects by name or path"
        counter={projects === null ? undefined : `${matches.length} of ${projects.length}`}
        onChange={(value) => {
          setQuery(value)
          setSelectedPath(null)
        }}
        onKeyDown={(ev) => {
          if (ev.nativeEvent.isComposing) return
          const action = listAction(ev, available.length, selectedIndex)
          if (!isListKey(action)) return
          ev.preventDefault()
          ev.stopPropagation()
          if (action.kind === 'dismiss') onDismiss()
          else if (action.kind === 'select') setSelectedPath(available[action.index]?.project.path ?? null)
          else if (action.kind === 'accept') open(available[selectedIndex])
        }}
        onDismiss={onDismiss}
        scrollRef={scrollRef}
        footer={
          <div className={styles.footer}>
            <Hint keys="↑↓">navigate</Hint>
            <Hint keys="⏎">open</Hint>
            <Hint keys="esc">dismiss</Hint>
            <Button variant="quiet" size="sm" icon="folder" onClick={() => {
              onDismiss()
              void browseForProject()
            }}>Open folder…</Button>
          </div>
        }
      >
        {failed ? (
          <PickerStatus>
            Could not load projects
            <Button variant="quiet" size="sm" onClick={() => setAttempt((value) => value + 1)}>Retry</Button>
          </PickerStatus>
        ) : projects === null ? (
          <PickerStatus>Loading projects…</PickerStatus>
        ) : projects.length === 0 ? (
          <PickerStatus>No recent projects — open a folder to get started</PickerStatus>
        ) : matches.length === 0 ? (
          <PickerStatus>No matching projects</PickerStatus>
        ) : matches.map((entry) => (
          <PickerRow
            key={entry.project.path}
            ref={(el) => {
              if (el === null) rowRefs.current.delete(entry.project.path)
              else rowRefs.current.set(entry.project.path, el)
            }}
            selected={entry.project.path === selected}
            disabled={!entry.exists}
            title={entry.exists ? entry.project.path : MISSING_REASON}
            data-project-path={entry.project.path}
            onMouseMove={() => { if (entry.exists) setSelectedPath(entry.project.path) }}
            onMouseDown={(ev) => {
              ev.preventDefault()
              if (ev.button === 0) open(entry)
            }}
          >
            <span className={styles.name}>{entry.project.name}</span>
            <span className={styles.path}>{entry.project.displayPath}</span>
            {!entry.exists && <span className={styles.missing}>Folder missing</span>}
          </PickerRow>
        ))}
      </ModalShell>
    </div>
  )
}
