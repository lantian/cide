// Mounted browser fixture: real file/comparison panes and menus, deterministic native boundary.
import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { EditorView } from '@codemirror/view'
import { EditorPane } from '@/panes/EditorPane'
import { ClipboardDiffPane } from '@/panes/ClipboardDiffPane'
import { installFakeTauri, emitEvent } from '@/demo/fakeTauri'
import { useWorkspace } from '@/store/workspace'
import { file, sharedDocument, clipboardComparison, clipboard, settings, diagnostics, symbols, git } from '@/ipc/client'
import { documentFor } from '@/editor/sharedDocuments'
import { sharedHistory } from '@/editor/sharedDocuments'
import '@/styles/fonts.css'
import '@/styles/tokens.css'
import '@/styles/iconMasks.css'
import '@/icons/icon.css'
import '@/editor/highlight.css'

installFakeTauri('shell:audit', new Map(), () => null)
const w = window as any
const path = '/project/example.ts'
const disk = 'export function greet(name: string) {\n  return `Hello, ${name}`\n}\n\n' + Array.from({length: 80}, (_, n) => `// context ${n + 1}`).join('\n')
let text = disk, stamp = { mtimeNanos: '1', len: text.length }, snapshot: any = null
let clip = disk.replace('Hello', 'Welcome')
const boot: any = { role: { kind: 'shell', active: 'p', projects: ['p'] }, workspace: {
  rev: 1, projects: { p: { id: 'p', tabs: [
    { id: 'file', kind: { kind: 'file', path, dirty: false }, tree: { panes: {} } },
    { id: 'compare', kind: { kind: 'clipboardDiff', path, snapshotId: 'clip', dirty: false }, tree: { panes: {} } },
  ], activeTab: 'file', roots: [{path:'/project'}], detached: {} } }, windows: {},
  settings: { theme: 'light', editor: { autosave: false, completion: false, diffView: 'split' }, inspections: {}, git: {} },
} }
useWorkspace.setState({ boot })
file.read = async () => ({ path, text, writable: true, stamp }) as any
file.write = async (_, next) => { text = next; stamp = { mtimeNanos: String(Number(stamp.mtimeNanos) + 1), len: next.length }; return stamp }
file.position = async () => null
file.backupRead = async () => null
file.setDirty = async (_, __, dirty) => { w.dirty = dirty; return { rev: 1 } }
settings.get = async () => boot.workspace.settings
settings.set = async patch => {
  boot.workspace.settings.editor = { ...boot.workspace.settings.editor, ...(patch as any).editor }
  emitEvent('cide://workspace-changed', boot.workspace)
  return boot.workspace.settings
}
symbols.outline = async () => ({ kind: 'ready', language: 'TypeScript', symbols: [], truncated: false })
diagnostics.get = async () => ({ kind: 'unavailable', reason: 'audit' }) as any
git.repos = async () => []
sharedDocument.sync = async (_, expected, next) => {
  if (next && (snapshot?.version ?? null) === expected) {
    snapshot = { ...next, version: (expected ?? 0) + 1 }
    queueMicrotask(() => emitEvent('cide://shared-document-changed', { path, version: snapshot.version }))
  }
  return snapshot
}
clipboard.readText = async () => clip
clipboardComparison.content = async () => clip
w.model = () => documentFor(path)
w.fileView = () => EditorView.findFromDOM(document.querySelector('#original .cm-editor')!)
w.diffView = () => EditorView.findFromDOM(document.querySelector('#comparison .cm-merge-b .cm-editor, #comparison .cm-editor:not([aria-label])')!)
w.saved = () => text
w.undoShared = () => sharedHistory(w.fileView(), 'undo')
w.theme = (theme: string) => { document.documentElement.dataset.theme = theme; useWorkspace.setState({ theme: theme as any }) }
function Fixture() {
  const [show, setShow] = useState(false)
  const [id, setId] = useState('clip')
  clipboardComparison.open = async () => { setShow(true); setId(String(Date.now())); return 'compare' }
  w.refreshClipboard = () => { clip = clip.replace('Welcome', 'Hi'); setId(String(Date.now())) }
  return <div style={{display:'grid', gridTemplateColumns: show ? '32% 68%' : '1fr', height:'100vh', fontFamily:'var(--font-ui)', background:'var(--bg)', color:'var(--text)'}}>
    <section id="original" style={{ minWidth:0, minHeight:0 }}><EditorPane path={path} project="p" tab="file" /></section>
    {show && <section id="comparison" style={{ minWidth:0, minHeight:0 }}><ClipboardDiffPane path={path} project="p" tab="compare" snapshotId={id} /></section>}
  </div>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
