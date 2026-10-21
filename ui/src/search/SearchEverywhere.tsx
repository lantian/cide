import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react'
import { Dialog } from '@/kit/components/Overlay'
import { Button, IconButton } from '@/kit/components/Button'
import { TextInput } from '@/kit/components/Field'
import { Tabs } from '@/kit/components/Surface'
import { Modal } from '@/overlays/ModalShell'
import { searchCommands } from '@/overlays/score'
import { useMergedContext } from '@/keys/context'
import { evaluateWhen } from '@/keys/when'
import type { OverlayHostProps } from '@/overlays/OverlayHost'
import { toolWindow, type SearchSession } from '@/ipc/client'
import { clearFocusRequest, useFocusRequested } from '@/chrome/focusRequests'
import { notifyFailure } from '@/chrome/notices'
import { selectTransientToolTab, searchTabId } from '@/toolwindow/selection'
import { location, MODE_LABELS, MODES, results, type Result, type SearchMode } from './model'
import { EMPTY_DRAFT, useSearchSessions, startSession, cancelSession, pinSession } from './store'
import { useWorkspace } from '@/store/workspace'
import { FolderScope } from './FolderScope'
import { ResultsView, type ResultNavigation } from './ResultsView'
import styles from './Search.module.css'

export function SearchEverywhere({ project, commands, context, keymap, actions, onDismiss }: OverlayHostProps & { onDismiss: () => void }) {
  const draft = useSearchSessions(s => s.drafts[project ?? ''] ?? EMPTY_DRAFT)
  const focus = useSearchSessions(s => s.focus)
  const wanted = useFocusRequested('search')
  const input = useRef<HTMLInputElement>(null)
  const root = useRef<HTMLDivElement>(null)
  const previousFocus = useRef<HTMLElement | null>(document.activeElement as HTMLElement | null)
  const firstProject = useRef(project)
  const mounted = useRef(true)
  const [session, setSession] = useState<SearchSession | null>(null)
  const [error, setError] = useState('')
  const [starting, setStarting] = useState(false)
  const [pinning, setPinning] = useState(false)
  const selected = useRef<ResultNavigation | null>(null)
  const layout = useWorkspace(s => s.boot?.workspace.settings.searchModalLayout ?? 'sideBySide')
  const patch = (next: Partial<typeof draft>) => useSearchSessions.getState().patchDraft(project, next)
  const descriptor = draft.descriptor
  const query = descriptor.query
  const mode = draft.mode
  const id = `modal:${project ?? ''}`
  const frame = useSearchSessions(s => session ? s.frames[session.id] ?? null : null)
  const requestError = useSearchSessions(s => session ? s.errors[session.id] ?? '' : '')
  const currentFrame = frame && JSON.stringify(frame.session.descriptor) === JSON.stringify(descriptor) ? frame : null
  const merged = useMergedContext(context)
  const commandRows = useMemo(() => mode === 'all' || mode === 'actions'
    ? searchCommands(commands.filter(command => command.unavailable !== null || evaluateWhen(command.when, merged)), query.pattern)
    : [], [commands, merged, mode, query.pattern])
  const items = useMemo(() => results(currentFrame, commandRows, keymap.chipFor), [currentFrame, commandRows, keymap])
  const setMode = (mode: SearchMode) => {
    if (pinning) return
    if (project === null && mode !== 'all' && mode !== 'actions') return
    patch({ mode, descriptor: { ...descriptor, source: mode === 'actions' ? 'all' : mode } })
    input.current?.focus()
  }
  const cycleMode = (direction: number) => {
    const modes = project === null ? ['all', 'actions'] as const : MODES
    const index = modes.indexOf(mode as never)
    setMode(modes[(index + direction + modes.length) % modes.length] ?? 'all')
  }
  const onKeyDownCapture = (e: KeyboardEvent) => {
    if (e.nativeEvent.isComposing || !e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault(); e.stopPropagation()
      cycleMode(e.key === 'ArrowLeft' ? -1 : 1)
    }
  }
  useLayoutEffect(() => {
    input.current?.focus(); input.current?.select()
    if (wanted) clearFocusRequest('search')
  }, [focus, wanted])
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      // An accepted result or action may already have focused its destination. Only restore
      // focus when dismissal has left it on the body (or inside the disappearing modal).
      queueMicrotask(() => {
        if (document.activeElement === document.body && previousFocus.current?.isConnected) previousFocus.current.focus()
      })
    }
  }, [])
  useEffect(() => { if (project !== firstProject.current) onDismiss() }, [project, onDismiss])
  useEffect(() => {
    let alive = true
    let owned: SearchSession | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    setSession(null); setError('')
    useSearchSessions.getState().patchView(id, { selected: null, collapsed: new Set(), scroll: 0 })
    const searchable = project !== null && mode !== 'actions' && (query.pattern !== '' || mode === 'files')
    setStarting(searchable)
    const start = async () => {
      if (!alive || project === null) return
      try {
        const next = await startSession(project, descriptor, () => alive)
        if (!alive || next === null) return
        owned = next; setSession(next); setStarting(false)
      } catch (e) {
        if (!alive) return
        const message = String(e)
        if (message.includes('still opening')) { timer = setTimeout(() => void start(), 400); return }
        setError(message); setStarting(false)
      }
    }
    if (searchable) timer = setTimeout(() => void start(), 140)
    return () => { alive = false; clearTimeout(timer); if (owned) cancelSession(owned) }
  }, [project, descriptor, mode, id])
  const open = (result: Result, modifier?: 'split' | 'mention' | 'session') => {
    if (result.kind === 'action') {
      if (result.command.unavailable !== null) return
      onDismiss()
      if (modifier === 'session') actions.runCommandInNewSession(result.command.id)
      else actions.runCommand(result.command.id)
      return
    }
    const at = location(result)
    if (!at) return
    onDismiss()
    if (result.kind === 'file') {
      if (modifier === 'split') actions.openFileInSplit(at.path)
      else if (modifier === 'mention') actions.mentionFile(at.path)
      else actions.openFile(at.path)
    } else actions.goToSymbol(at.path, at.line, at.column, at.endColumn)
  }
  const canPin = session !== null && JSON.stringify(session.descriptor) === JSON.stringify(descriptor)
    && mode !== 'actions' && query.pattern.trim() !== '' && !starting
    && (mode !== 'text' || currentFrame?.text?.error == null)
  const pin = async () => {
    if (!canPin || !session || pinning) return
    setPinning(true)
    try {
      const tab = await pinSession(session)
      const view = useSearchSessions.getState().views[id]
      if (view) useSearchSessions.getState().patchView(tab.id, view)
      if (!mounted.current) return
      selectTransientToolTab(project, searchTabId(tab.id))
      await toolWindow.setLayout(project, { open: true })
      if (mounted.current) onDismiss()
    } catch (e) { setError(String(e)); notifyFailure(e) }
    finally { setPinning(false) }
  }
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isPropagationStopped() || e.nativeEvent.isComposing) return
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (!pinning) onDismiss(); return }
    if (e.key === 'Tab' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault()
      cycleMode(e.shiftKey ? -1 : 1)
      return
    }
    if (e.key === 'Tab') {
      const focusable = Array.from(root.current?.closest('[role="dialog"]')?.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled):not([tabindex="-1"]), [tabindex="0"]') ?? [])
        .filter(el => el.getClientRects().length > 0)
      const first = focusable[0], last = focusable.at(-1)
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus() }
      if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus() }
    }
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault(); e.stopPropagation()
      if (mode === 'actions') selected.current?.open('session')
      else void pin()
      return
    }
    if (e.target === input.current) {
      if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey
        && selected.current?.expand(e.key === 'ArrowRight')) e.preventDefault()
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); selected.current?.move(e.key === 'ArrowDown' ? 1 : -1) }
      if (e.key === 'PageDown' || e.key === 'PageUp') { e.preventDefault(); selected.current?.page(e.key === 'PageDown' ? 1 : -1) }
      if (e.key === 'Enter') { e.preventDefault(); selected.current?.open(e.shiftKey ? 'split' : e.altKey ? 'mention' : undefined) }
    }
  }
  const changeQuery = (patchQuery: Partial<typeof query>) => patch({ descriptor: { ...descriptor, query: { ...query, ...patchQuery } } })
  const problem = currentFrame?.text?.error
  const status = [error, requestError, ...(currentFrame?.errors ?? []), problem?.detail].filter(Boolean).join(' · ')
  const textOptions = mode === 'text' || mode === 'all'
  const total = (currentFrame?.files?.matched ?? 0) + (currentFrame?.symbols?.matched ?? 0) + (currentFrame?.text?.total ?? 0) + commandRows.length
  const truncated = (currentFrame?.files && currentFrame.files.matched > currentFrame.files.items.length)
    || (currentFrame?.symbols && (currentFrame.symbols.truncated || currentFrame.symbols.matched > currentFrame.symbols.items.length)) || currentFrame?.text?.truncated
  return <Modal onDismiss={() => { if (!pinning) onDismiss() }}>
    <Dialog title="Search Everywhere" width="search" flush onKeyDown={onKeyDown} onKeyDownCapture={onKeyDownCapture} onClose={() => { if (!pinning) onDismiss() }}>
      <div ref={root} className={styles.modal} data-audit="searchEverywhere">
        <Tabs label="Search modes" value={mode} onChange={setMode} tabs={MODES.filter(m => project !== null || m === 'all' || m === 'actions').map(value => ({ value, label: MODE_LABELS[value] }))} />
        <div className={styles.controls}>
          <TextInput ref={input} icon="search" value={query.pattern} placeholder={mode === 'text' ? 'Search in files' : 'Search everywhere'} aria-label="Search query" data-audit="searchInput"
            spellCheck={false} autoComplete="off" disabled={pinning} invalid={problem?.kind === 'pattern'} onChange={e => changeQuery({ pattern: e.target.value })} />
          <Button size="sm" icon="panel-bottom" disabled={!canPin} busy={pinning} title={mode === 'actions' ? 'Actions cannot be pinned' : 'Pin code results (Ctrl+Enter)'} onClick={() => void pin()}>Pin results</Button>
        </div>
        {textOptions && <div className={styles.filters} role="group" aria-label="Text search options">
          {mode === 'all' && <span className={styles.meta}>Text:</span>}
          <IconButton icon="case-sensitive" label="Match case" pressed={query.caseSensitive} disabled={pinning} onClick={() => changeQuery({ caseSensitive: !query.caseSensitive })} />
          <IconButton icon="whole-word" label="Whole word" pressed={query.wholeWord} disabled={pinning} onClick={() => changeQuery({ wholeWord: !query.wholeWord })} />
          <IconButton icon="regex" label="Regular expression" pressed={query.mode === 'regex'} disabled={pinning} onClick={() => changeQuery({ mode: query.mode === 'regex' ? 'literal' : 'regex' })} />
          <FolderScope project={project} value={query.scope} disabled={pinning} invalid={problem?.kind === 'scope'} onChange={scope => changeQuery({ scope })} />
          <TextInput size="sm" value={query.include} aria-label="File name pattern" placeholder="File patterns, e.g. *.ts" disabled={pinning} invalid={problem?.kind === 'include'} onChange={e => changeQuery({ include: e.target.value })} />
        </div>}
        {(mode === 'files' || mode === 'all') && <div className={styles.filters}>
          <Button size="sm" variant="quiet" aria-pressed={descriptor.libraries} disabled={pinning || project === null} onClick={() => patch({ descriptor: { ...descriptor, libraries: !descriptor.libraries } })}>Files: {descriptor.libraries ? 'Project + external libraries' : 'Project only'} (Alt+L)</Button>
        </div>}
        <ResultsView project={project} layout={layout} id={id} items={items} mode={mode} running={starting || currentFrame?.running === true} error={status} onOpen={open} onMode={setMode} selectedRef={selected} />
        <div className={styles.footer} aria-live="polite">
          <span>{starting || currentFrame?.running ? 'Searching…' : `${total.toLocaleString()} results`}{truncated ? ' · Result limit reached' : ''}</span>
          <span>↑/↓ / PgUp / PgDn Preview · ←/→ Groups · Shift+←/→ Mode · Enter Open · {mode === 'actions' ? 'Ctrl+Enter New session' : 'Ctrl+Enter Pin'} · Esc Close</span>
        </div>
      </div>
    </Dialog>
  </Modal>
}
