import type { KeymapReport } from '../../ipc/generated'
import type { Scene } from '../scenes'
import { sleep, until } from '../drive'
import { leaf, pane, tab } from '../world'
import { keymapReport } from '../data/keymap'

/** Settings ▸ Keymap, filtered to the pane commands a tiling-grid user rebinds first, so both the default and the user layer are on screen. */
export const keymap: Scene = {
  setup: (world, handlers) => {
    // A settings tab rather than the frame: the frame draws only with no project open.
    const host = pane('editor', 'settings')
    world.open(tab({ kind: 'settings', section: 'keymap' }, leaf(host.id), [host]))
    handlers.set('keymap_report', (): KeymapReport => keymapReport(world.boot.keymap))
  },
  drive: async () => {
    // Typed the way a person types it: through the input's own value setter and an `input`
    // event, which is what React's controlled field listens for — assigning `.value` alone would
    // be overwritten on the next render.
    const box = () => document.querySelector<HTMLInputElement>('input[aria-label="Filter keybindings"]')
    await until(() => box() !== null)
    const input = box()
    if (!input) return
    input.focus()
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(input, 'pane.')
    input.dispatchEvent(new Event('input', { bubbles: true }))
    await sleep(200)
  },
}
