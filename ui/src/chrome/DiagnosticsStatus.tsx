import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { events, languageServers, type ProjectId } from '@/ipc/client'
import type { LanguageServerSnapshot } from '@/ipc/generated'
import { Icon } from '@/icons/Icon'
import { useContextMenu } from '@/menus'
import type { MenuEntry } from '@/menus/model'
import { useWorkspace } from '@/store/workspace'
import { buildKeymap } from '@/keys/keymap'
import type { DiagnosticsSnapshot } from '@/sidebar/ProblemsPanel/model'
import { diagnosticIndicator } from '@/toolwindow/problemsModel'
import { openServerLogs } from '@/toolwindow/selection'
import { notifyFailure } from './notices'
import styles from './DiagnosticsStatus.module.css'

const EMPTY_BINDINGS: never[] = []
function serverState(row: LanguageServerSnapshot): 'ready' | 'scanning' | 'unavailable' | 'stopped' { return row.stopped ? 'stopped' : row.status.kind }
function serverDetail(row: LanguageServerSnapshot): string {
  const state = row.stopped ? 'Stopped' : row.status.kind === 'ready' ? 'Running' : row.status.kind === 'scanning' ? row.status.detail || 'Starting…' : row.status.reason
  const memory = row.memoryBytes === null ? '' : row.memoryBytes >= 1024 ** 3 ? `${(row.memoryBytes / 1024 ** 3).toFixed(1)} GiB` : `${Math.round(row.memoryBytes / 1024 ** 2)} MiB`
  return [state, row.version, memory].filter(Boolean).join(' — ')
}

export function DiagnosticsStatus({ project, projectName, snapshot, onOpenProblems }: { project: ProjectId | null; projectName: string; snapshot: DiagnosticsSnapshot; onOpenProblems: () => void }) {
  const [result, setResult] = useState<{ project: ProjectId; rows: LanguageServerSnapshot[]; error: string | null } | null>(null)
  const [pendingProject, setPendingProject] = useState<ProjectId | null>(null)
  const pending = project !== null && pendingProject === project
  const projectRef = useRef(project)
  projectRef.current = project
  const button = useRef<HTMLButtonElement>(null)
  const generation = useRef(0)
  const rows = result?.project === project ? result.rows : null
  const error = result?.project === project ? result.error : null
  const bindings = useWorkspace(s => s.boot?.keymap ?? EMPTY_BINDINGS)
  const keymap = useMemo(() => buildKeymap(bindings), [bindings])
  const refresh = useCallback(async () => {
    if (!project || projectRef.current !== project) return
    const stamp = ++generation.current
    try {
      const next = await languageServers.get(project)
      if (stamp === generation.current && projectRef.current === project) setResult({ project, rows: next, error: null })
    } catch (e) {
      if (stamp === generation.current && projectRef.current === project) setResult(old => ({ project, rows: old?.project === project ? old.rows : [], error: String(e) }))
    }
  }, [project])
  useEffect(() => {
    void refresh()
    const off = events.onDiagnostics(id => { if (id === project) void refresh() })
    return () => { generation.current++; void off.then(stop => stop()) }
  }, [project, refresh])
  const act = useCallback((source: string | null, action: 'stop' | 'restart') => {
    if (!project || pending) return
    setPendingProject(project)
    void languageServers.control(project, source, action).then(refresh).catch(notifyFailure).finally(() => setPendingProject(current => current === project ? null : current))
  }, [project, pending, refresh])
  const items = useCallback((): MenuEntry[] => [
    { id: 'project', label: projectName || 'No project open', information: true },
    ...(error ? [{ id: 'error', label: error, information: true }] : []),
    ...(rows ?? []).map(row => ({
      id: row.source, label: row.label, status: serverState(row),
      submenu: (): MenuEntry[] => [
        { id: 'logs', label: 'View Logs', run: () => { if (project) void openServerLogs(project, row.source, row.label).catch(notifyFailure) } },
        { id: 'restart', label: row.attached ? 'Reconnect Server' : 'Restart Server', ...(pending ? { disabledReason: 'An operation is in progress' } : { run: () => act(row.source, 'restart') }) },
        { id: 'stop', label: row.attached ? 'Disconnect Server' : 'Stop Server', ...(pending || row.stopped ? { disabledReason: row.stopped ? 'Already stopped' : 'An operation is in progress' } : { run: () => act(row.source, 'stop') }) },
        { kind: 'separator' },
        { id: 'status', label: serverDetail(row), information: true, status: serverState(row) },
      ],
    })),
    ...(!rows?.length && !error ? [{ id: 'empty', label: rows === null && project ? 'Reading language servers…' : 'No language servers for this project', information: true }] : []),
    { kind: 'separator' },
    { id: 'restart-all', label: 'Restart All Servers', ...(project && rows?.length && !pending ? { run: () => act(null, 'restart') } : { disabledReason: pending ? 'An operation is in progress' : 'No servers available' }) },
    { id: 'stop-all', label: 'Stop All Servers', ...(project && rows?.some(row => !row.stopped) && !pending ? { run: () => act(null, 'stop') } : { disabledReason: pending ? 'An operation is in progress' : 'No active servers' }) },
  ], [project, projectName, rows, error, pending, act])
  const menu = useContextMenu({ label: 'Language Servers', items, live: true, keymap })
  useEffect(() => {
    if (!menu.isOpen) return
    const timer = setInterval(() => { void refresh() }, 1000)
    return () => clearInterval(timer)
  }, [menu.isOpen, refresh])
  const openMenu = useCallback((target: HTMLElement) => {
    const rect = target.getBoundingClientRect()
    menu.openAt(rect.left, rect.top, target)
  }, [menu.openAt])
  useEffect(() => { menu.close() }, [project, menu.close])
  useEffect(() => {
    const open = () => { if (button.current) openMenu(button.current) }
    window.addEventListener('cide:language-servers', open)
    return () => window.removeEventListener('cide:language-servers', open)
  }, [openMenu])
  const operational = !!rows?.length && rows.every(row => !row.stopped && row.status.kind === 'ready')
  const scanning = rows?.some(row => !row.stopped && row.status.kind === 'scanning')
  const serverSummary = error ?? (pending ? 'Updating language servers…' : operational ? 'All servers operational' : scanning ? 'Language servers are working…' : rows?.length ? 'Some servers are stopped or unavailable' : rows === null && project ? 'Reading language servers…' : 'No language servers configured')
  const status = diagnosticIndicator(snapshot)
  const { counts } = status
  const minor = counts.info + counts.hint + counts.other
  const summary = [`${counts.error} errors, ${counts.warning} warnings${minor ? `, ${minor} informational findings` : ''}`, status.scanning ? 'Analysis in progress' : '', status.partial ? 'Analysis unavailable or incomplete' : '', snapshot.kind === 'ready' && snapshot.truncated ? `${snapshot.truncated} additional findings omitted` : ''].filter(Boolean).join('\n')
  const problemsKey = keymap.chipFor('sidebar.problems')
  const serversKey = keymap.chipFor('languageServers.open')
  return <div className={styles.controls} data-audit="diagnosticsStatus">
    <button ref={button} type="button" className={`${styles.button} ${scanning ? styles.busy : ''}`} aria-label="Language Servers" aria-haspopup="menu" aria-expanded={menu.isOpen} title={`Language Servers${serversKey ? `   ${serversKey}` : ''}\n${serverSummary}`} onClick={e => menu.isOpen ? menu.close() : openMenu(e.currentTarget)}>
      <Icon name="zap" size={1} />
    </button>
    <button type="button" className={styles.button} aria-label={`Project Diagnostics: ${project ? summary : 'No project open'}`} title={`Project Diagnostics${problemsKey ? `   ${problemsKey}` : ''}\n${project ? summary : 'No project open'}`} disabled={!project} onClick={onOpenProblems}>
      {counts.error > 0 && <span className={`${styles.count} ${styles.error}`}><Icon name="circle-x" size={0} />{counts.error}</span>}
      {counts.warning > 0 && <span className={`${styles.count} ${styles.warning}`}><Icon name="triangle-alert" size={0} />{counts.warning}</span>}
      {counts.error + counts.warning === 0 && minor > 0 && <span className={styles.count}><Icon name="info" size={0} />{minor}</span>}
      {status.scanning ? <span className={styles.busy}><Icon name="loader-circle" size={0} /></span> : status.partial ? <Icon name="info" size={0} /> : status.clean ? <Icon name="check" size={1} /> : counts.error + counts.warning + minor === 0 ? <Icon name="minus" size={0} /> : null}
    </button>
    {menu.menu}
  </div>
}
