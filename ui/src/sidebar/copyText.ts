/**
 * Put a string on the system clipboard, from a context menu item.
 *
 * Three sidebar menus offer *Copy path* and one offers *Copy match*, so the fallback below is
 * written once rather than three times.
 *
 * # Why there is a fallback at all
 *
 * `navigator.clipboard.writeText` is the right call and it is the one that runs. It is also
 * the one that can reject: it requires a secure context, and it is gated on transient user
 * activation, which a menu item click has but which is not something this module can prove
 * from here. WebKitGTK has historically been the strict one about both. The old
 * `document.execCommand('copy')` path has neither requirement, so it is what the rejection
 * falls back to.
 *
 * Note which direction this goes. `menus/native.ts` explains why a hand-rolled *Paste* is not
 * offered anywhere in this app — WebKit refuses `execCommand('paste')` from page script, so
 * the item would be a dead control. Copy is the opposite case: `execCommand('copy')` is
 * allowed, because writing the user's own selection to their own clipboard leaks nothing.
 *
 * A failure is reported through `diag.log` and answered as `false`. A silent no-op is the
 * failure mode this codebase keeps having to fix — from the outside, "copied nothing" and
 * "menu item wired to nothing" are the same thing.
 */
import { diag } from '@/ipc/client'

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch (error) {
    if (legacyCopy(text)) return true
    void diag.log(`[cide] clipboard write failed: ${String(error)}`)
    return false
  }
}

/**
 * The pre-async-clipboard path: a detached textarea, selected, copied, removed.
 *
 * `position: fixed` with a zero opacity rather than `display: none` — a hidden element cannot
 * hold a selection, so the usual "just hide it" version of this silently copies nothing. It
 * is on screen for the length of one synchronous function and never paints.
 */
function legacyCopy(text: string): boolean {
  if (typeof document === 'undefined') return false
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.top = '0'
  area.style.left = '0'
  area.style.width = '1px'
  area.style.height = '1px'
  area.style.opacity = '0'
  document.body.appendChild(area)
  try {
    area.select()
    return document.execCommand('copy')
  } catch {
    return false
  } finally {
    area.remove()
  }
}
