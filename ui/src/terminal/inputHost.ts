/// <reference types="vite/client" />
/**
 * The DOM half of `inputRouting.ts`: one capture-phase `input` listener per pane.
 *
 * # Why the listener goes on the host element, not the textarea
 *
 * xterm registers its own `input` handler on `term.textarea` in `_initGlobal`, i.e. on the
 * *target*. A capture listener on any ancestor runs before the target's own listeners, and
 * `host.el` is the element `term.open()` was given — so it is an ancestor of the textarea by
 * construction. That ordering is what makes `stopPropagation()` here deterministically
 * disarm `_inputEvent`; a listener on the textarea itself would be racing registration
 * order, and xterm's was registered first.
 *
 * # Why the textarea is cleared
 *
 * `CompositionHelper._handleAnyTextareaChanges` snapshots `textarea.value`, waits a
 * `setTimeout(0)`, and emits `newValue.replace(oldValue, '')`. Leaving a committed character
 * sitting in the textarea leaves that diff armed with content, so a later unrelated change
 * emits the accumulated remainder. Clearing after every routed insertion keeps the snapshot
 * base empty, which is the only value for which the diff is harmless.
 */
import type { TerminalHandle } from './xterm'

/**
 * Route this pane's `input` events through the terminal's router.
 *
 * Returns the removal function; the caller puts it in `host.cleanup` so `teardown` takes it
 * down with everything else.
 */
export function attachInputRouting(handle: TerminalHandle, el: HTMLElement): () => void {
  const listener = (raw: Event) => {
    const ev = raw as InputEvent
    const decision = handle.input.input(
      { inputType: ev.inputType, data: ev.data, isComposing: ev.isComposing },
      performance.now(),
    )
    if (decision === 'defer') return

    // Both remaining outcomes stop the event: `_inputEvent` must not be the one that decides.
    ev.stopPropagation()
    if (decision === 'emit' && ev.data !== null) {
      // xterm's own "as if typed" path, the same one the Shift+Enter re-encoding uses, so
      // the bytes leave through the `onData` the pane already listens on and typing's side
      // effects (scroll to bottom, clear selection) still happen.
      handle.term.input(ev.data, true)
    }
    const textarea = handle.term.textarea
    if (textarea) textarea.value = ''
  }

  el.addEventListener('input', listener, true)
  return () => el.removeEventListener('input', listener, true)
}

/**
 * Whether this window was launched with the input probe turned on.
 *
 * Read from the URL, which is how every other opt-in instrument in this app is switched on
 * (`bench`, `audit`, `panes`, `windows` — all set by `crates/cide-app/src/windows.rs` from an
 * env var). A query parameter rather than `localStorage` because the reason this probe exists
 * at all is that devtools cannot be reached from a shell on Wayland, so a switch that needs
 * devtools to flip is no switch.
 */
function probeRequested(): boolean {
  if (!import.meta.env.DEV) return false
  try {
    return new URLSearchParams(window.location.search).get('inputprobe') === '1'
  } catch {
    // No `location` at all (a non-browser import). Not asked for, then.
    return false
  }
}

/**
 * A keystroke-level trace of every emitter, for the one question no reading of the source
 * can settle: whether this machine's input method re-delivers ordinary ASCII at all.
 *
 * Development builds, and **only when asked for**: `./run.sh --input-probe`, which sets
 * `CIDE_INPUT_PROBE=1`, which puts `inputprobe=1` on the window URL. On by default it would
 * be its own bug — it writes one `diag_log` per keydown, keyup, `input` and `onData`, and
 * `diag_log` is a synchronous command, so that is four main-thread IPC round trips and four
 * file appends per character typed, on the thread that receives the keystrokes. Instrumenting
 * a lag report with something that causes lag measures the instrument.
 *
 * It does **not** use `console.log`: `cmd/diag.rs` already documents that the webview console
 * is unreachable from a shell on Wayland, which is the environment this is for. It goes to the
 * Rust log, one line per event, timestamped from `performance.now()` so the keyup/commit race
 * is legible in the ordering.
 *
 * Reading it:
 *
 * * a real `keyCode` plus a `data` line arriving *after* that key's `keyup` — the async
 *   IM-commit path this whole module exists for; the `swallow` decision on the same line is
 *   the fix working.
 * * `keyCode 229` on keydown — the `CompositionHelper` snapshot-diff path; the keydown is
 *   returned false in `xterm.ts` and the `emit` here is the only delivery.
 * * one `data` line per keystroke and no `input` lines at all — no IM involvement; the
 *   duplication, if any, is not here.
 */
export function attachInputProbe(
  handle: TerminalHandle,
  el: HTMLElement,
  paneId: string,
  log: (line: string) => void,
): () => void {
  if (!probeRequested()) return () => {}

  const textarea = handle.term.textarea
  if (!textarea) return () => {}

  const tag = paneId.slice(0, 8)
  const at = () => performance.now().toFixed(1)
  const removers: Array<() => void> = []

  const on = <K extends keyof HTMLElementEventMap>(
    type: K,
    fn: (ev: HTMLElementEventMap[K]) => void,
    target: HTMLElement = textarea,
  ) => {
    const wrapped = (ev: Event) => fn(ev as HTMLElementEventMap[K])
    // Capture, so the trace records what arrived rather than what survived xterm.
    target.addEventListener(type, wrapped, true)
    removers.push(() => target.removeEventListener(type, wrapped, true))
  }

  on('keydown', (ev) => {
    log(
      `input ${tag} ${at()} keydown key=${JSON.stringify(ev.key)} code=${ev.code} ` +
        `keyCode=${ev.keyCode} composing=${ev.isComposing} claims=${handle.input.pending()}`,
    )
  })
  on('keyup', (ev) => log(`input ${tag} ${at()} keyup key=${JSON.stringify(ev.key)}`))
  // On the host element, not the textarea, and registered before `attachInputRouting` — that
  // listener calls `stopPropagation()` on the same element in the same phase, and an event it
  // swallows never reaches the textarea at all. A probe that traced only the events the fix
  // let through would show one emitter per keystroke however broken the routing was, which is
  // the one answer this probe must never be able to give.
  on(
    'input',
    (raw) => {
      const ev = raw as InputEvent
      log(
        `input ${tag} ${at()} input type=${ev.inputType} data=${JSON.stringify(ev.data)} ` +
          `composed=${ev.composed} claims=${handle.input.pending()}`,
      )
    },
    el,
  )
  for (const type of ['compositionstart', 'compositionupdate', 'compositionend'] as const) {
    on(type, (ev) => log(`input ${tag} ${at()} ${type} data=${JSON.stringify(ev.data)}`))
  }

  const onData = handle.term.onData((data) => log(`input ${tag} ${at()} onData ${JSON.stringify(data)}`))
  removers.push(() => onData.dispose())

  return () => {
    for (const fn of removers) fn()
  }
}
