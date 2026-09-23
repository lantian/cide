/**
 * Opening a web link, from anywhere that shows one.
 *
 * Terminal output (a bare `https://…` or an OSC 8 hyperlink), a markdown preview and an OpenSpec
 * document all end here, and the open itself is Rust's `app_open_url` — see its doc comment for
 * why it is not a JS-side `window.open` (that would navigate this app's own webview) nor the JS
 * opener plugin (capability-gated per window, so it would silently do nothing in a detached
 * pane). Rust also owns the scheme allowlist, so a surface that forgets to filter still cannot
 * hand `file:` or `javascript:` to the desktop.
 *
 * Until this existed every one of those surfaces refused with "copy the address", and a bare URL
 * ctrl+clicked in a terminal fell through to the file-path matcher and was reported as a path
 * that names no file in the project — which is what the user actually saw.
 */
import { notify } from '@/chrome/notices'
import { app } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'

/** Whether `url` is the kind of link [`openWebLink`] will pass on. The same rule Rust applies. */
export function isWebUrl(url: string): boolean {
  return /^https?:\/\/[^\s/]/i.test(url.trim())
}

/**
 * Open `url` in the default browser, or say why not.
 *
 * Success is the browser coming up, so it gets no notice; a refusal or a desktop with no browser
 * does, because a click that was taken and answered by nothing is the defect this project keeps
 * finding.
 */
export function openWebLink(url: string): void {
  app.openUrl(url).catch((reason: unknown) => notify(errorText(reason), { kind: 'warn' }))
}
