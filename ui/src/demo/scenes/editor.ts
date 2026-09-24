import type { CompletionAnswer, DiagnosticsSnapshot } from '../../ipc/generated'
import type { Scene } from '../scenes'
import { fileTab } from '../world'
import { sleep, until } from '../drive'
import { COMPLETION, SESSION_PATH, SESSION_POSITION, sessionDiagnostics, sessionFileHandlers } from '../data/editor'

/**
 * The code editor on `session.rs`, mid-edit: Rust highlighting, the change column against HEAD,
 * rust-analyzer's squiggles, and Ctrl+Space's popup with an auto-import row at the top.
 *
 * The caret is placed by the remembered view position (`file_position`) rather than by a click,
 * because that is the door a reopened file really comes through — and it lands the caret behind
 * `Credit` with no driving at all.
 */
export const editor: Scene = {
  setup: (world, handlers) => {
    world.open(fileTab(SESSION_PATH, true))
    for (const [cmd, h] of sessionFileHandlers(SESSION_POSITION)) handlers.set(cmd, h)
    handlers.set('diagnostics_get', (): DiagnosticsSnapshot => ({
      kind: 'ready',
      source: 'rust-analyzer',
      items: sessionDiagnostics(),
      sources: [],
      truncated: 0,
    }))
    handlers.set('diagnostics_completion', (): CompletionAnswer => COMPLETION)
  },
  // The keys go to whatever holds focus, and a file opened by a restored workspace does not take
  // it — a click in the text would, but it would also move the caret the position put there.
  // `preventScroll`, because a bare `focus()` scrolls the content element's top into view and
  // throws away the remembered scroll, popup and all.
  drive: async () => {
    await until(() => document.querySelector('.cm-content') !== null)
    ;(document.querySelector('.cm-content') as HTMLElement | null)?.focus({ preventScroll: true })
    // The grammar highlights in the background, and a headless page starves it; a picture taken
    // before it reaches the viewport is a screen of plain text. Wait until the caret's line is
    // coloured.
    await until(() => document.querySelector('.cm-activeLine .cide-tk-keyword') !== null)
    await sleep(200)
  },
  keys: ['ctrl+space'],
}
