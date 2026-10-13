/**
 * Keep Excalidraw's own export actions, but give their browser clipboard writes a native
 * destination. The context menu in 0.18.1 holds the original action objects, so replacing
 * actions with `registerAction` would fix the shortcut while leaving the reported menu broken.
 * Installing before the lazy import also fixes its module-level clipboard capability probes.
 *
 * Window lifetime, not pane lifetime: hidden tabs stay mounted, exports are asynchronous, and
 * tearing down on one tab's unmount would break another tab's pending copy. Clipboard reads
 * are untouched; unsupported write formats still use the original browser implementation.
 * Native functions are injected so this adapter can be exercised without Tauri or Excalidraw.
 */
export interface ClipboardPixels {
  readonly width: number
  readonly height: number
  readonly rgba: Uint8Array
}

export interface DrawingClipboard {
  readonly writeText: (text: string) => Promise<void>
  readonly writeImage: (image: ClipboardPixels) => Promise<void>
}

const INSTALLED = Symbol.for('cide.excalidrawClipboard')
type ClipboardHost = Window & typeof globalThis & {
  [INSTALLED]?: { backend: DrawingClipboard }
}

/** Supply the item interface only when WebKit does not expose it. Never replace native items. */
class DrawingClipboardItem {
  readonly types: readonly string[]
  readonly presentationStyle = 'unspecified'

  constructor(private readonly data: Record<string, Blob | string | PromiseLike<Blob | string>>) {
    this.types = Object.keys(data)
    if (this.types.length === 0) throw new TypeError('A clipboard item needs a format')
  }

  async getType(type: string): Promise<Blob> {
    if (!Object.hasOwn(this.data, type)) throw new DOMException(`No ${type} clipboard data`, 'NotFoundError')
    const value = await this.data[type]
    return typeof value === 'string' ? new Blob([value], { type }) : value!
  }

  static supports(type: string): boolean {
    return type === 'image/png' || type === 'text/plain'
  }
}

/** Decode with the webview's existing PNG decoder; free the object URL and canvas on all exits. */
export async function pngPixels(blob: Blob, host: Window & typeof globalThis = window): Promise<ClipboardPixels> {
  const url = host.URL.createObjectURL(blob)
  const image = new host.Image()
  const canvas = host.document.createElement('canvas')
  try {
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve()
      image.onerror = () => reject(new Error('The clipboard PNG could not be decoded'))
      image.src = url
    })
    const width = image.naturalWidth
    const height = image.naturalHeight
    if (width === 0 || height === 0) throw new Error('The clipboard PNG is empty')
    canvas.width = width
    canvas.height = height
    const context = canvas.getContext('2d')
    if (context === null) throw new Error('A canvas is unavailable for copying the PNG')
    context.drawImage(image, 0, 0)
    const { data } = context.getImageData(0, 0, width, height)
    return { width, height, rgba: new Uint8Array(data.buffer, data.byteOffset, data.byteLength) }
  } finally {
    image.onload = null
    image.onerror = null
    image.removeAttribute('src')
    host.URL.revokeObjectURL(url)
    canvas.width = 0
    canvas.height = 0
  }
}

export function installExcalidrawClipboard(backend: DrawingClipboard, host: ClipboardHost = window): void {
  const installed = host[INSTALLED]
  if (installed !== undefined) {
    installed.backend = backend
    return
  }
  const state = { backend }
  const clipboard = host.navigator.clipboard ?? {} as Clipboard
  const originalWrite = clipboard.write?.bind(clipboard)
  if (typeof host.ClipboardItem === 'undefined') {
    Object.defineProperty(host, 'ClipboardItem', { configurable: true, value: DrawingClipboardItem })
  }
  Object.defineProperties(clipboard, {
    writeText: { configurable: true, value: (text: string) => state.backend.writeText(text) },
    write: {
      configurable: true,
      value: async (items: ClipboardItem[]): Promise<void> => {
        const item = items[0]
        if (items.length === 1 && item?.types.length === 1 && item.types[0] === 'image/png') {
          await state.backend.writeImage(await pngPixels(await item.getType('image/png'), host))
        } else if (items.length === 1 && item?.types.length === 1 && item.types[0] === 'text/plain') {
          await state.backend.writeText(await (await item.getType('text/plain')).text())
        } else if (originalWrite !== undefined) {
          await originalWrite(items)
        } else {
          throw new DOMException('This clipboard format is not supported', 'NotSupportedError')
        }
      },
    },
  })
  if (host.navigator.clipboard !== clipboard) {
    Object.defineProperty(host.navigator, 'clipboard', { configurable: true, value: clipboard })
  }
  host[INSTALLED] = state
}

/** Bubble after the drawing engine handled the gesture, shielding cide's enclosing pane menu. */
export function claimExcalidrawContextMenu(event: { stopPropagation: () => void }): void {
  event.stopPropagation()
}
