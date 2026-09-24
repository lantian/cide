/**
 * What a scene's `drive()` uses to put the booted app into the state its picture is of.
 *
 * Only through the app's own request registries (`chrome/panelRequests.ts`) — the same doors a
 * command-palette entry or a status-bar click goes through — and never by reaching into a
 * component's state, so a scene cannot show a state the app has no way to arrive at.
 */
import { panelHostPresent, requestPanel, requestSettingsFrame } from '../chrome/panelRequests'
import type { PanelView } from '../chrome/sidebarView'

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** Wait until `test` holds, polling each frame; give up quietly after `ms`. */
export async function until(test: () => boolean, ms = 5000): Promise<boolean> {
  const end = Date.now() + ms
  while (!test()) {
    if (Date.now() > end) return false
    await sleep(50)
  }
  return true
}

/** Show a sidebar panel, as clicking its rail icon would. */
export async function showPanel(view: PanelView): Promise<void> {
  await until(panelHostPresent)
  requestPanel(view)
  await sleep(400)
}

/**
 * Open the settings *frame* on a section. It only draws while no project is active (`App.tsx`
 * renders it for `activeProject === null`), so in the demo world — where `cide` always is — a
 * scene wants a settings **tab** instead: `world.open(tab({ kind: 'settings', section }, …))`,
 * which is what Rust opens from the gear menu when a project is open.
 */
export async function showSettings(section: string | null): Promise<void> {
  await until(() => requestSettingsFrame(section))
  await sleep(400)
}

/** Click the first element matching `selector`, if there is one. */
export async function click(selector: string): Promise<boolean> {
  const found = await until(() => document.querySelector(selector) !== null, 3000)
  ;(document.querySelector(selector) as HTMLElement | null)?.click()
  await sleep(300)
  return found
}
