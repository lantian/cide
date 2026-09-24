import type { SearchFrame, SearchQuery, ViewPosition } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import type { Scene } from '../scenes'
import { fileTab, wire } from '../world'
import { showPanel, sleep, until } from '../drive'
import { HITS, PATTERN, frame } from '../data/search'
import { SESSION_CLEAN, SESSION_PATH, sessionFileHandlers } from '../data/editor'

/**
 * The ⌕ panel with `backpressure` typed in, and a populated list of matches across the PTY
 * crate, its neighbours and the docs — beside `session.rs`, open on one of the hits.
 *
 * Search is a poll (`search_query` with an advancing offset), not a Channel, so one handler
 * answers it: every hit on the first page, `running: false`, and the panel stops asking.
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
    handlers.set('search_query', (a: Args): SearchFrame => frame(a['query'] as SearchQuery, Number(a['offset'] ?? 0)))
    handlers.set('search_cancel', () => true)
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
