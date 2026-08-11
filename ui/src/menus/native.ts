/**
 * Kills the webview's own context menu, app-wide.
 *
 * `devtools` is enabled in this build, so WebKitGTK's default menu now carries "Inspect
 * element" — on a tab strip, on a file tree row, on the terminal. That is a browser leaking
 * through a desktop app, and it is what the user asked to be rid of.
 *
 * **One listener, in the capture phase, on `window`.** The alternative — every surface
 * calling `preventDefault()` in its own `onContextMenu` — is the one that cannot work: it is
 * correct only for surfaces someone remembered, so the default is "leaks", and the next pane
 * anyone adds leaks again. Capture also means it does not matter whether a surface handles
 * the event at all, or in what order; by the time the event has been dispatched anywhere the
 * default is already prevented, and WebKit only consults that flag afterwards.
 *
 * The escape hatch is `data-native-menu` and the text-input default, both decided by
 * `wantsNativeMenu` in `model.ts` — read the long comment there for what was chosen for
 * `<input>` and why.
 */
import { NATIVE_MENU_ATTR, wantsNativeMenu, type ElementFacts } from './model'

export interface SuppressionOptions {
  /**
   * Let text fields keep the webview's menu, so cut/copy/paste survive. Default `true`; see
   * `wantsNativeMenu`.
   */
  readonly nativeInTextInputs?: boolean | undefined
}

/** Where the uninstaller is parked, so a Vite hot reload cannot leave two listeners behind. */
const INSTALLED = Symbol.for('cide.nativeMenuSuppression')

type Host = typeof globalThis & { [INSTALLED]?: (() => void) | undefined }

/** Read one element into the flat record `wantsNativeMenu` decides from. */
function factsOf(el: Element): ElementFacts {
  const facts: {
    tag: string
    type?: string
    nativeMenu?: string
    contentEditable?: boolean
    disabled?: boolean
  } = { tag: el.tagName.toLowerCase() }

  const stated = el.getAttribute(NATIVE_MENU_ATTR)
  if (stated !== null) facts.nativeMenu = stated
  if (el instanceof HTMLElement && el.isContentEditable) facts.contentEditable = true
  if (el instanceof HTMLInputElement) {
    facts.type = el.type
    if (el.disabled) facts.disabled = true
  } else if (el instanceof HTMLTextAreaElement && el.disabled) {
    facts.disabled = true
  }
  return facts
}

function chainOf(target: EventTarget | null): ElementFacts[] {
  const chain: ElementFacts[] = []
  let el = target instanceof Element ? target : null
  while (el !== null) {
    chain.push(factsOf(el))
    el = el.parentElement
  }
  return chain
}

/**
 * Install the global suppression. Idempotent — calling it twice replaces the first listener
 * rather than stacking a second — and returns the uninstaller.
 *
 * A no-op outside a DOM, so importing `@/menus` from an SSR check script is harmless.
 */
export function installNativeMenuSuppression(options: SuppressionOptions = {}): () => void {
  const host = globalThis as Host
  host[INSTALLED]?.()

  if (typeof document === 'undefined' || typeof window === 'undefined') {
    const noop = () => {}
    host[INSTALLED] = undefined
    return noop
  }

  const nativeInTextInputs = options.nativeInTextInputs ?? true

  const onContextMenu = (ev: Event) => {
    if (wantsNativeMenu(chainOf(ev.target), nativeInTextInputs)) return
    ev.preventDefault()
  }

  window.addEventListener('contextmenu', onContextMenu, true)
  const uninstall = () => {
    window.removeEventListener('contextmenu', onContextMenu, true)
    host[INSTALLED] = undefined
  }
  host[INSTALLED] = uninstall
  return uninstall
}
