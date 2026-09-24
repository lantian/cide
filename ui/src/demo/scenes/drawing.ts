import type { Scene } from '../scenes'
import { fileTab } from '../world'
import { sleep, until } from '../drive'
import { DRAWING_PATH, NOTE_PATH, drawingHandlers } from '../data/drawing'

/**
 * A `.excalidraw` file opened as a drawing: the real `ExcalidrawPane` over the branch's design
 * diagram — claude PTY → Coalescer → Channel → xterm.js, and the ack loop back.
 *
 * The design note it belongs to sits in the tab beside it, open on its rendered preview. Only one
 * of them can be on screen: a file tab's editor path is the *tab's* (`App.tsx`, `filePath`), and
 * splitting a file tab makes a Claude pane rather than a second document, so there is no layout
 * that puts a markdown preview and a drawing pane side by side. The drawing is the one a picture
 * can sell; the note's tab still says it is there.
 */
export const drawing: Scene = {
  setup: (world, handlers) => {
    world.open(fileTab(NOTE_PATH))
    world.open(fileTab(DRAWING_PATH))
    for (const [cmd, h] of drawingHandlers()) handlers.set(cmd, h)
  },
  // Excalidraw arrives as a lazy chunk, then fetches its hand-drawn face, then paints the scene
  // on a canvas — three waits, and a picture taken after the first is an empty grid. Wait for
  // the font, then for the static canvas to hold something other than its background.
  drive: async () => {
    await until(() => document.querySelector('.excalidraw canvas.static') !== null, 15000)
    await until(() => document.fonts.check('20px Excalifont'), 10000)
    await document.fonts.ready
    await until(() => painted(), 10000)
    await sleep(400)
  },
}

/** Whether the static canvas has any ink on it: some pixel differs from the top-left one. */
function painted(): boolean {
  const canvas = document.querySelector<HTMLCanvasElement>('.excalidraw canvas.static')
  const ctx = canvas?.getContext('2d')
  if (!canvas || !ctx || canvas.width === 0) return false
  const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height)
  for (let i = 4; i < data.length; i += 4 * 97) {
    if (data[i] !== data[0] || data[i + 1] !== data[1] || data[i + 2] !== data[2]) return true
  }
  return false
}
