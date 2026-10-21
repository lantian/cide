/** Two isolated modifier taps, not two shifted keystrokes. The first tap passes through;
 * ordinary typing, composition, a held/repeated key, pointer input and blur cancel it. */
export interface ShiftTapEvent {
  type?: string | undefined
  key: string
  code: string
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
  repeat?: boolean | undefined
  isComposing?: boolean | undefined
}
export function createDoubleShift(now: () => number, interval = 400) {
  let down = false
  let pressedAt = 0
  let previous: number | null = null
  const reset = () => { down = false; previous = null }
  return {
    reset,
    feed(event: ShiftTapEvent): boolean {
      const shift = event.code === 'ShiftLeft' || event.code === 'ShiftRight' || event.key === 'Shift'
      if (!shift || event.ctrlKey || event.altKey || event.metaKey || event.repeat || event.isComposing) { reset(); return false }
      if ((event.type ?? 'keydown') === 'keydown') {
        if (down) { reset(); return false }
        down = true; pressedAt = now(); return false
      }
      if (event.type !== 'keyup' || !down) return false
      down = false
      const time = now()
      if (time - pressedAt > interval) { reset(); return false }
      if (previous !== null && time - previous <= interval) { previous = null; return true }
      previous = time
      return false
    },
  }
}
