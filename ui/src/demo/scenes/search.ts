import type { SearchDescriptor, SearchSession, SearchSessionFrame, ViewPosition } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { fileTab, wire, uid } from '../world'
import { showPanel, sleep, until } from '../drive'
import { HITS, PATTERN, frame } from '../data/search'
import { SESSION_CLEAN, SESSION_PATH, sessionFileHandlers } from '../data/editor'

/**
 * The Text mode of Search Everywhere, with code preview beside the matches. The session
 * handlers also support pinning, refreshing and reopening the modal in the interactive demo.
 */
export const search: Scene = {
  setup: (world, handlers) => {
    world.open(fileTab(SESSION_PATH))
    // Dragged wider, as anyone reading results does: at the default width every line is cut off
    // before its match. The files panel shares the width (`--w-sidebar-files`).
    world.boot.workspace.settings.sidebar.filesWidth = 440
    // Scrolled to the `PtyError::Backpressure` hit, as if it had been opened from the list.
    const line = HITS.find((h) => h.rel.endsWith('session.rs') && h.text.includes('PtyError'))?.line ?? 1
    const at: ViewPosition = { path: SESSION_PATH, topLine: Math.max(1, line - 20), line, column: 36, folds: [], markdownView: 'text', touchedAt: wire(0) }
    for (const [cmd, h] of sessionFileHandlers(at, SESSION_CLEAN)) handlers.set(cmd, h)
    const sessions = new Map<string, SearchSession>()
    handlers.set('search_session_start', (a: Args): SearchSession => {
      const session: SearchSession = { id: uid('search'), project: String(a['project']), descriptor: a['descriptor'] as SearchDescriptor, pinned: false, generation: 0 }
      for (const old of sessions.values()) if (!old.pinned) sessions.delete(old.id)
      sessions.set(session.id, session)
      return session
    })
    const get = (a: Args) => { const session = sessions.get(String(a['session'])); if (!session) throw new Error('Search closed'); return session }
    handlers.set('search_session_page', (a: Args): SearchSessionFrame => {
      const session = get(a)
      return { session, files: null, symbols: null, text: frame(session.descriptor.query, Number(a['offset'] ?? 0)), running: false, errors: [] }
    })
    handlers.set('search_session_pin', (a: Args) => { const session = get(a); session.pinned = true; return session })
    handlers.set('search_session_rerun', (a: Args) => { const session = get(a); session.generation++; return session })
    handlers.set('search_session_list', () => [...sessions.values()].filter(s => s.pinned))
    handlers.set('search_session_close', (a: Args) => { sessions.delete(String(a['session'])) })
    handlers.set('search_session_cancel', (a: Args) => { if (!sessions.get(String(a['session']))?.pinned) sessions.delete(String(a['session'])) })
  },
  drive: async () => {
    await showPanel('search')
    await until(() => document.querySelector('input[placeholder="Search in files"]') !== null)
    const input = document.querySelector<HTMLInputElement>('input[placeholder="Search in files"]')
    if (!input) return
    // Typed the way React sees typing: the native value setter, then an `input` event. Assigning
    // `.value` alone updates the DOM behind React's back and `onChange` never fires.
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set
    setter?.call(input, PATTERN)
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await until(() => (document.body.textContent ?? '').includes(HITS[1]?.text.trim().slice(0, 30) ?? ''))
    await sleep(300)
  },
}
