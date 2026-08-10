/// <reference types="vite/client" />
/**
 * The DOM half of `inputRouting.ts`: four listeners per pane, and the code that empties the
 * textarea.
 *
 * # Why the listeners go on the host element, not the textarea
 *
 * xterm registers its own handlers on `term.textarea` in `_initGlobal` (lines 378-384), i.e.
 * on the *target*. A capture listener on any ancestor runs before every listener on the
 * target, whatever order they were registered in, and `host.el` is the element `term.open()`
 * was given — so it is an ancestor of the textarea by construction. That ordering is the
 * whole mechanism:
 *
 * * `keydown` in **capture** runs before `_keyDown`, so the textarea is empty before
 *   `CompositionHelper` snapshots `oldValue` (line 187) and before a `compositionstart`
 *   records `_compositionPosition.start` (line 63). Every base is taken against `''`.
 * * `input` in **capture** lets `stopPropagation()` deterministically disarm `_inputEvent`.
 * * `compositionend` in **bubble**, uniquely, because it must run *after* xterm's listener:
 *   `_finalizeComposition` queues a `setTimeout(0)` that reads the composed text out of the
 *   textarea, and the clear that follows has to be queued behind that read, not in front of
 *   it. A capture listener here would empty the textarea before the commit was taken out of
 *   it and CJK input would silently vanish.
 *
 * # Why clearing the textarea is safe at all
 *
 * Nothing in xterm 6 reads the textarea for display or for accessibility: `_syncTextArea`
 * (line 301) only moves and resizes it, and the value is written only by the browser's own
 * editing, by `paste`, and by `onLinuxMouseSelection`. It is an input scratchpad, and the
 * only code that treats it as a buffer is the code with the bug.
 */
import type { Reaction } from './inputRouting'
import type { TerminalHandle } from './xterm'

/**
 * Attach this pane's textarea discipline.
 *
 * Returns the removal function; the caller puts it in `host.cleanup` so `teardown` takes it
 * down with everything else.
 */
export function attachInputRouting(handle: TerminalHandle, el: HTMLElement): () => void {
  const guard = handle.input

  const apply = (r: Reaction, ev: Event | null): void => {
    if (r.stop) ev?.stopPropagation()
    // Cleared before the write, so anything the write reaches synchronously — `onUserInput`
    // clearing the selection, the scroll-to-bottom — already sees a settled textarea.
    if (r.clear) {
      const textarea = handle.term.textarea
      if (textarea) textarea.value = ''
    }
    if (r.emit !== null) {
      // xterm's own "as if typed" path, the same one the Shift+Enter re-encoding uses, so
      // the bytes leave through the `onData` the pane already listens on and typing's side
      // effects (scroll to bottom, clear selection) still happen.
      handle.term.input(r.emit, true)
    }
    if (r.defer) setTimeout(() => apply(guard.commitSettled(), null), 0)
  }

  const onKeydown = (raw: Event): void => {
    const ev = raw as KeyboardEvent
    apply(
      guard.keydown({
        key: ev.key,
        keyCode: ev.keyCode,
        ctrlKey: ev.ctrlKey,
        altKey: ev.altKey,
        metaKey: ev.metaKey,
        isComposing: ev.isComposing,
      }),
      ev,
    )
  }

  const onInput = (raw: Event): void => {
    const ev = raw as InputEvent
    apply(
      guard.input({ inputType: ev.inputType, data: ev.data, isComposing: ev.isComposing }),
      ev,
    )
  }

  const onCompositionStart = (ev: Event): void => apply(guard.compositionstart(), ev)
  const onCompositionEnd = (ev: Event): void => apply(guard.compositionend(), ev)

  el.addEventListener('keydown', onKeydown, true)
  el.addEventListener('input', onInput, true)
  el.addEventListener('compositionstart', onCompositionStart, true)
  // Bubble. Not a slip — see the module comment; this one has to follow xterm's.
  el.addEventListener('compositionend', onCompositionEnd, false)

  return () => {
    el.removeEventListener('keydown', onKeydown, true)
    el.removeEventListener('input', onInput, true)
    el.removeEventListener('compositionstart', onCompositionStart, true)
    el.removeEventListener('compositionend', onCompositionEnd, false)
  }
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
 * A keystroke-level trace of every emitter, for the questions no reading of the source can
 * settle: what this machine's input method actually sends, and what is in the textarea when
 * it sends it.
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
 * Rust log, one line per event, timestamped from `performance.now()` so the commit/keyup race
 * is legible in the ordering.
 *
 * Reading it — `ta=` is the field that matters, because the whole defect was textarea
 * residue:
 *
 * * `ta=` non-zero on a keydown — the invariant is broken; something is writing to the
 *   textarea that this module does not know about, and every base-relative emitter in xterm
 *   is about to be wrong by exactly that much.
 * * `onData` longer than the character typed — one of the textarea emitters swept residue
 *   into its payload. Compare it against the preceding `ta=`.
 * * two `onData` lines for one keystroke — two emitters fired; the `composing=`/`commit=`
 *   fields say which window the second one came from.
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
  const guardState = () => {
    const s = handle.input.state()
    return `ta=${JSON.stringify(textarea.value)} composing=${s.composing} commit=${s.commitPending}`
  }

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

  // On the host element, not the textarea, and registered before `attachInputRouting` — those
  // listeners are on the same element in the same phase, they empty the textarea and they call
  // `stopPropagation()`. Registered the other way round the probe would report a textarea that
  // had just been cleared and would never see a swallowed event at all, which is the one
  // answer this probe must never be able to give.
  on('keydown', (ev) => {
    log(
      `input ${tag} ${at()} keydown key=${JSON.stringify(ev.key)} code=${ev.code} ` +
        `keyCode=${ev.keyCode} composing=${ev.isComposing} ${guardState()}`,
    )
  }, el)
  on('keyup', (ev) => log(`input ${tag} ${at()} keyup key=${JSON.stringify(ev.key)}`), el)
  on(
    'input',
    (raw) => {
      const ev = raw as InputEvent
      log(
        `input ${tag} ${at()} input type=${ev.inputType} data=${JSON.stringify(ev.data)} ` +
          `composed=${ev.composed} ${guardState()}`,
      )
    },
    el,
  )
  for (const type of ['compositionstart', 'compositionupdate', 'compositionend'] as const) {
    on(type, (ev) => log(`input ${tag} ${at()} ${type} data=${JSON.stringify(ev.data)} ${guardState()}`), el)
  }

  const onData = handle.term.onData((data) => log(`input ${tag} ${at()} onData ${JSON.stringify(data)}`))
  removers.push(() => onData.dispose())

  return () => {
    for (const fn of removers) fn()
  }
}
