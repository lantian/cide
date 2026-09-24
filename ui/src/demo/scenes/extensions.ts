import type { ExtensionPage, ExtensionSnapshot } from '../../ipc/generated'
import type { Scene } from '../scenes'
import { showPanel } from '../drive'
import { leaf, pane, tab } from '../world'
import { godotPage, snapshot } from '../data/extensions'

/**
 * The Extensions panel over two marketplaces, with the Godot extension's page open beside it —
 * the README, the permissions in words, and the buttons that act on it.
 */
export const extensions: Scene = {
  setup: (world, handlers) => {
    const base = world.boot.extensions
    const current = (): ExtensionSnapshot => snapshot(base)
    // The resolved registry rides the bootstrap too, so the language table the editor builds at
    // boot agrees with the one the panel lists.
    world.boot.extensions = current().resolved
    handlers.set('ext_snapshot', current)
    handlers.set('ext_reload', current)
    handlers.set('ext_page', (): ExtensionPage => godotPage())
    // An editor pane, as `ext_open_tab` gives one: the page renders over it.
    const host = pane('editor', 'Godot')
    world.open(tab({ kind: 'extension', marketplace: 'community', extension: 'godot', name: 'Godot' }, leaf(host.id), [host]))
  },
  drive: async () => {
    await showPanel('extensions')
  },
}
