/** File views share this session locally; compare-and-swap keeps detached windows coherent. */
import { Annotation, Prec, StateEffect, Transaction } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { events, file, sharedDocument, type FileStamp } from '@/ipc/client'
import { notify } from '@/chrome/notices'
import { SharedDocumentModel, changesBetween } from './sharedDocumentModel'

const mirrored = Annotation.define<boolean>()
interface Session {
  path: string
  tabs: Set<string>
  model: SharedDocumentModel
  ready: Promise<void>
  flushing: Promise<void> | null
  views: Set<EditorView>
  retired: boolean
  saving: Promise<FileStamp | null> | null
  scheduled: ReturnType<typeof setTimeout> | null
}
const sessions = new Map<string, Session>()
const attached = new WeakMap<EditorView, Session>()
let listening: Promise<unknown> | null = null

function listen(): void {
  if (!listening) void events.onWorkspaceChanged(workspace => {
    const tabs = Object.values(workspace.projects).flatMap(p => p.tabs.flatMap(t =>
      t.kind.kind === 'file' || t.kind.kind === 'clipboardDiff' ? [{ id: t.id, path: t.kind.path }] : []))
    const paths = new Set(tabs.map(t => t.path))
    for (const [path, session] of sessions) if (!paths.has(path)) {
      sessions.delete(path)
      const renamed = tabs.find(t => session.tabs.has(t.id))
      if (renamed) {
        session.path = renamed.path
        sessions.set(renamed.path, session)
        void flushDocument(renamed.path).catch(report)
      } else session.retired = true
    }
  }).catch(report)
  listening ??= sharedDocument.onChanged(({ path, version }) => {
    const session = sessions.get(path)
    if (!session || version <= (session.model.version ?? 0)) return
    // A pending CAS consumes the new state itself. Fetching it here as well could acknowledge
    // an operation twice, so remote notifications join that queue instead of racing it.
    void refresh(session).catch(report)
  }).catch(report)
}

function report(error: unknown): void {
  notify(`The shared editor could not synchronize: ${String(error)}`, { kind: 'error' })
}

async function refresh(session: Session): Promise<void> {
  await session.ready
  if (session.flushing) await session.flushing
  if (session.retired) return
  if (session.model.hasPending) await flushDocument(session.path)
  const snapshot = await sharedDocument.sync(session.path, null, null)
  if (session.flushing) { await session.flushing; return refresh(session) }
  if (snapshot && snapshot.version > (session.model.version ?? 0)) session.model.receive(snapshot)
}

export async function prepareDocument(path: string, text: string, stamp: FileStamp | null, tab?: string): Promise<SharedDocumentModel> {
  let session = sessions.get(path)
  if (!session) {
    listen()
    const model = new SharedDocumentModel(text, stamp)
    session = { path, tabs: new Set(), model, ready: Promise.resolve(), flushing: null, views: new Set(), retired: false, saving: null, scheduled: null }
    sessions.set(path, session)
    session.ready = sharedDocument.sync(path, null, model.snapshot()).then(snapshot => {
      if (snapshot) model.receive(snapshot)
    }).catch(error => {
      if (sessions.get(path) === session) sessions.delete(path)
      throw error
    })
  }
  if (tab) session.tabs.add(tab)
  await session.ready
  return session.model
}

export function documentFor(path: string): SharedDocumentModel | undefined { return sessions.get(path)?.model }

export async function flushDocument(path: string): Promise<void> {
  const session = sessions.get(path)
  if (!session || session.retired) return
  if (session.scheduled !== null) { clearTimeout(session.scheduled); session.scheduled = null }
  await session.ready
  if (session.flushing) { await session.flushing; return flushDocument(session.path) }
  session.flushing = (async () => {
    while (!session.retired && session.model.hasPending) {
      const sent = session.model.submission()
      const sentPath = session.path
      const result = await sharedDocument.sync(sentPath, sent.expected, sent.next)
      if (!result && session.path !== sentPath) continue
      if (!result) return // The last tab closed; its backup/close guard owns the contents now.
      const accepted = result.version === (sent.expected ?? 0) + 1 && result.state === sent.next.state
        && result.savedText === sent.next.savedText && JSON.stringify(result.stamp) === JSON.stringify(sent.next.stamp)
      session.model.receive(result, accepted ? sent.count : 0)
    }
  })()
  try { await session.flushing } finally { session.flushing = null }
}

/** Finish pending edits before a tab/window hands its views to another renderer. */
export async function flushDocuments(): Promise<void> {
  await Promise.all([...sessions.keys()].map(flushDocument))
}

export function sharedHistory(view: EditorView, direction: 'undo' | 'redo'): boolean | null {
  const session = attached.get(view)
  if (!session) return null
  if (view.state.readOnly || !session.model.history(direction)) return false
  view.dispatch({ selection: session.model.state.selection, scrollIntoView: true })
  return true
}
export function sharedHistoryDepth(view: EditorView, direction: 'undo' | 'redo'): number | null {
  const model = attached.get(view)?.model
  return model ? direction === 'undo' ? model.undoDepth : model.redoDepth : null
}

/** Called after the surface has restored its recovered text, before handing out Save. */
export function attachDocument(
  path: string, view: EditorView,
  baseline: (text: string) => void,
): () => void {
  const session = sessions.get(path)
  if (!session) return () => {}
  session.views.add(view)
  attached.set(view, session)
  const update = (source?: unknown) => {
    const text = session.model.state.doc.toString()
    if (source !== view && text !== view.state.doc.toString()) {
      view.dispatch({ changes: changesBetween(view.state.doc.toString(), text),
        annotations: [mirrored.of(true), Transaction.addToHistory.of(false)] })
    }
    baseline(session.model.savedText)
    // Local peers update in this transaction; batch native snapshots once per frame.
    session.scheduled ??= setTimeout(() => {
      session.scheduled = null
      void flushDocument(session.path).catch(report)
    }, 16)
  }
  const unsubscribe = session.model.subscribe(update)
  view.dispatch({ effects: StateEffect.appendConfig.of([
    Prec.highest(keymap.of([
      { key: 'Mod-z', run: () => sharedHistory(view, 'undo') ?? false },
      { key: 'Mod-Shift-z', run: () => sharedHistory(view, 'redo') ?? false },
      { key: 'Mod-y', run: () => sharedHistory(view, 'redo') ?? false },
    ])),
    EditorView.updateListener.of(event => {
      for (const tr of event.transactions) {
        if (tr.docChanged && !tr.annotation(mirrored)) session.model.edit(tr, view)
      }
    }),
  ]) })
  // Recovery is an edit over the disk baseline, never a new saved baseline.
  if (session.views.size === 1 && session.model.version === 1 && !session.model.dirty
    && view.state.doc.toString() !== session.model.state.doc.toString()) {
    session.model.edit(session.model.state.update({
      changes: changesBetween(session.model.state.doc.toString(), view.state.doc.toString()),
      userEvent: 'input.recover',
    }), view)
  }
  update()
  return () => {
    unsubscribe()
    attached.delete(view)
    session.views.delete(view)
    // A reload/remount in this turn reuses the same history. A closed final view releases
    // the renderer copy; the process copy survives detach until its workspace view closes.
    setTimeout(() => {
      if (session.views.size === 0 && sessions.get(session.path) === session) {
        void flushDocument(session.path).finally(() => {
          if (session.retired && session.views.size === 0 && sessions.get(session.path) === session) sessions.delete(session.path)
        }).catch(report)
      }
    }, 0)
  }
}

export async function saveSharedDocument(path: string, text: string, stamp: FileStamp | null): Promise<FileStamp | null> {
  const session = sessions.get(path)
  while (session?.saving) await session.saving
  const saving = save(path, text, stamp)
  if (session) session.saving = saving
  try { return await saving } finally { if (session?.saving === saving) session.saving = null }
}

async function save(path: string, text: string, stamp: FileStamp | null): Promise<FileStamp | null> {
  await flushDocument(path)
  const session = sessions.get(path)
  if (session) await refresh(session)
  const model = sessions.get(path)?.model
  const current = model?.state.doc.toString()
  if (current !== undefined && current !== text.replace(/\r\n?/g, '\n')) {
    throw new Error('The file changed while preparing to save. Save again to write the current content.')
  }
  if (model && !model.dirty) return model.stamp
  const result = await file.write(path, text, stamp)
  sessions.get(path)?.model.saved(text, result)
  await flushDocument(path)
  return result
}
