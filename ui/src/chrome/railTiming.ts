/**
 * How long a switch on the activity rail actually took, on the machine it happened on.
 *
 * # Why this exists
 *
 * Freeze work in this app has a record of not being checked: M31 calls its lag work "unmeasured",
 * and M38 and M102 both shipped without being confirmed on a display. Nothing in CI can see a frame
 * WebKitGTK did not paint, so without a number from a real run each round is a theory about which
 * code looks expensive. This prints the number.
 *
 * Dev builds only: the one call site is behind `import.meta.env.DEV`, so a release build drops it.
 *
 * # What the two figures mean
 *
 *     [cide] rail files→git: paint 38ms, worst frame 62ms
 *
 * **`paint`** is the click until the *second* animation frame. The first is not the answer: a
 * frame's rAF callbacks run before that frame's layout, its `ResizeObserver` delivery and its
 * paint, so the first callback proves only that React finished. The second is the first moment the
 * panel is known to have reached the screen.
 *
 * **`worst frame`** is the longest gap between consecutive frames in the second after the click,
 * and it is the figure that keeps the first one honest. A rail switch defers its terminal refits
 * until after the paint (`App.tsx`, `layout/resizeGesture.ts`), so `paint` alone would report a
 * stall that had merely been moved 130 ms later as a win. A freeze is a freeze whichever side of
 * the paint it lands on.
 *
 * # Why `console.warn`
 *
 * It is the quietest level that reaches the log: `ipc/consoleBridge.ts` forwards `warn` and `error`
 * only, for the reasons its header gives. Printed once the second is over, never during it, because
 * the bridge is a synchronous round trip per line and would otherwise be measured by the very line
 * reporting the measurement.
 *
 * Import-free, like the other small modules in this directory.
 */

/** How long frames are watched after a click. Long enough to cover a settle and its trickle. */
const SAMPLE_MS = 1000

/** Stops the sampling in progress, so a second click in the window reports only itself. */
let stopSampling: (() => void) | null = null

/** Start timing a switch from one rail view to another. `null` is a hidden sidebar. */
export function timeRailSwitch(from: string | null, to: string | null): void {
  stopSampling?.()
  const clicked = performance.now()
  let last = clicked
  let worst = 0
  let frames = 0
  let paint = 0
  let handle = requestAnimationFrame(function frame() {
    const now = performance.now()
    worst = Math.max(worst, now - last)
    last = now
    frames += 1
    if (frames === 2) paint = now - clicked
    if (now - clicked < SAMPLE_MS) {
      handle = requestAnimationFrame(frame)
      return
    }
    stopSampling = null
    console.warn(
      `[cide] rail ${from ?? 'hidden'}→${to ?? 'hidden'}: `
        + `paint ${Math.round(paint)}ms, worst frame ${Math.round(worst)}ms`,
    )
  })
  stopSampling = () => cancelAnimationFrame(handle)
}
