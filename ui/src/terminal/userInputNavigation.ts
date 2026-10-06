import type { TerminalHandle } from './xterm'
import { inputLocations, visibleInputOrdinal, type InputLocation } from './userInputModel'
import type { PromptEntry } from '@/panes/recapModel'

export type RevealResult = 'shown' | 'missing' | 'unsupported' | 'cancelled'

/** Viewport-owned navigation, independent of decorations. Native fullscreen history is
 * owned by the CLI; ask xterm to encode only the wheel reports that CLI has requested. */
export function followUserInputs(handle: TerminalHandle, harness: 'claude' | 'codex',
  initial: readonly PromptEntry[], onFollow: (ordinal: number | null) => void) {
  const term = handle.term
  const element = term.element
  let inputs = initial, locations: InputLocation[] = []
  let frame = 0, disposed = false, following = true, altScrolled = false, ownWheel = false, dirty = true
  let viewport = term.buffer.active.viewportY, base = term.buffer.active.baseY
  let context: number | undefined, direction = 0, revision = 0, jump = 0

  function read(): void {
    const buffer = term.buffer.active
    const lines = Array.from({ length: buffer.length }, (_, row) => {
      const line = buffer.getLine(row)
      return { text: line?.translateToString(true) ?? '', wrapped: line?.isWrapped ?? false }
    })
    locations = inputLocations(lines, inputs, harness, buffer.baseY + buffer.cursorY)
    dirty = false
  }
  function refresh(): void {
    frame = 0
    if (disposed) return
    const buffer = term.buffer.active
    const oldLocations = locations
    const parsed = dirty
    if (dirty) read()
    let ordinal = visibleInputOrdinal(locations, buffer.viewportY)
    // On an alternate screen an anchor may disappear just above the grid after scrolling
    // down. Remember that boundary until another confirmed prompt supplies the next one.
    if (buffer.type === 'alternate' && ordinal === undefined && direction > 0 && oldLocations.length) {
      ordinal = oldLocations.at(-1)?.ordinal
    }
    if (ordinal !== undefined) context = ordinal
    if (following && (buffer.type === 'normal' || altScrolled)) {
      if (buffer.type === 'normal' && buffer.viewportY === buffer.baseY) onFollow(null)
      else if (ordinal !== undefined) onFollow(ordinal >= (inputs.at(-1)?.ordinal ?? 0) ? null : ordinal)
    }
    viewport = buffer.viewportY
    base = buffer.baseY
    if (buffer.type === 'normal' || parsed) direction = 0
  }
  function schedule(): void { if (!frame && !disposed) frame = requestAnimationFrame(refresh) }
  function interact(event: Event): void {
    if (ownWheel) return
    jump++
    if (event instanceof WheelEvent) {
      if (event.ctrlKey || event.deltaY === 0) return
      direction = Math.sign(event.deltaY)
      following = true
      altScrolled = true
      schedule()
    } else if (event instanceof MouseEvent && (event.target as Element)?.closest('.xterm-viewport')) {
      following = true
    } else if (event instanceof KeyboardEvent && ['PageUp', 'PageDown', 'Home', 'End'].includes(event.key)) {
      following = true
      altScrolled = true
      direction = ['PageUp', 'Home'].includes(event.key) ? -1 : 1
      schedule()
    }
  }
  const subscriptions = [
    term.onWriteParsed(() => { revision++; dirty = true; schedule() }),
    term.onResize(() => { dirty = true; schedule() }),
    term.onScroll(() => {
      const buffer = term.buffer.active
      // Scrolling caused by incoming output moves base and viewport together. Public
      // scrollToLine, the scrollbar and wheel input instead move the viewport alone.
      if (buffer.viewportY !== viewport && buffer.viewportY - viewport !== buffer.baseY - base) following = true
      schedule()
    }),
    term.buffer.onBufferChange(() => { locations = []; context = undefined; altScrolled = false; dirty = true; schedule() }),
  ]
  for (const name of ['wheel', 'keydown', 'mousedown']) element?.addEventListener(name, interact, true)
  read()

  return {
    update(next: readonly PromptEntry[]): void { inputs = next; dirty = true; schedule() },
    pause(): void { following = false; jump++ },
    async reveal(ordinal: number): Promise<RevealResult> {
      if (disposed) return 'cancelled'
      const ticket = ++jump
      following = false
      // Flush a pending follow callback before starting the explicit jump.
      if (frame) { cancelAnimationFrame(frame); frame = 0 }
      read()
      const buffer = term.buffer.active
      if (buffer.type === 'normal') {
        const location = locations.find(input => input.ordinal === ordinal)
        if (!location) return 'missing'
        term.scrollToLine(location.start)
        // scrollToLine emits synchronously. Keep the selected recap while its own prompt
        // is on screen; resume automatic following at the next console scroll.
        following = false
        viewport = buffer.viewportY
        base = buffer.baseY
        term.focus()
        return 'shown'
      }
      let unchanged = 0
      const started = performance.now()
      for (let attempt = 0; attempt < 400 && performance.now() - started < 15000; attempt++) {
        if (disposed || ticket !== jump) return 'cancelled'
        read()
        if (locations.some(input => input.ordinal === ordinal)) { term.focus(); return 'shown' }
        const mode = term.modes.mouseTrackingMode
        if (!element || !['vt200', 'drag', 'any'].includes(mode) || term.buffer.active.type !== 'alternate') return 'unsupported'
        const screen = element.querySelector<HTMLElement>('.xterm-screen')
        const rect = screen?.getBoundingClientRect()
        if (!screen || !rect?.width || !rect.height) return 'unsupported'
        const before = revision
        const grid = (): string => Array.from({ length: term.rows }, (_, row) => term.buffer.active.getLine(row)?.translateToString(true) ?? '').join('\n')
        const previous = grid()
        const first = locations[0]?.ordinal ?? context ?? inputs.at(-1)?.ordinal ?? ordinal
        // With no visible anchor, context is the prompt above the grid. Revealing that
        // very input still requires scrolling upwards, rather than oscillating below it.
        const sign = ordinal <= first ? -1 : 1
        const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, view: window,
          clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2,
          deltaY: sign * handle.cellSize().height * 3, deltaMode: WheelEvent.DOM_DELTA_PIXEL })
        const answered = new Promise<void>(resolve => {
          const timer = setTimeout(() => { subscription.dispose(); resolve() }, 180)
          const subscription = term.onWriteParsed(() => {
            clearTimeout(timer)
            subscription.dispose()
            // A hidden window can suspend animation frames. PTY navigation must still
            // settle and honour cancellation when the user leaves this pane.
            setTimeout(resolve, 20)
          })
        })
        ownWheel = true
        try {
          // Keep samples overlapping even when the CLI scrolls three lines per report.
          for (let i = 0; i < Math.max(1, Math.min(6, Math.floor(term.rows / 6))); i++) screen.dispatchEvent(event)
        } finally { ownWheel = false }
        // Let the CLI answer through its PTY; never inject arrow keys or rewrite its grid.
        await answered
        if (disposed || ticket !== jump) return 'cancelled'
        if (revision === before || grid() === previous) unchanged++
        else unchanged = 0
        if (unchanged >= 3) return 'missing'
      }
      return 'missing'
    },
    dispose(): void {
      disposed = true
      jump++
      if (frame) cancelAnimationFrame(frame)
      for (const subscription of subscriptions) subscription.dispose()
      for (const name of ['wheel', 'keydown', 'mousedown']) element?.removeEventListener(name, interact, true)
    },
  }
}

export type InputNavigator = ReturnType<typeof followUserInputs>
