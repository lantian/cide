import type { AgentModels, ColorScheme, GraphicsStatus, Settings, SettingsPatch } from '../../ipc/generated'
import type { Args } from '../fakeTauri'
import { emitEvent } from '../fakeTauri'
import type { Scene } from '../scenes'
import { click, sleep, until } from '../drive'
import { leaf, pane, tab, wire } from '../world'
import { GRAPHICS, IMPORTED, SCHEMES } from '../data/settings-scheme'

/**
 * Settings ▸ Appearance: the theme, and colour schemes converted from VS Code themes.
 *
 * The picture is taken just after pressing **Import…** on a `.vsix` that carries a dark and a
 * light variant, so the row shows the affordance, the note about the half that went to the other
 * theme, and the picker open over every scheme imported so far.
 */
export const settingsScheme: Scene = {
  setup: (world, handlers) => {
    world.boot.schemes = structuredClone(SCHEMES)
    const settings = world.boot.workspace.settings
    settings.editor.colorSchemeDark = 'tokyo-night'
    settings.editor.colorSchemeLight = 'github-light-default'
    // A settings tab rather than the frame: the frame draws only with no project open.
    const host = pane('editor', 'settings')
    world.open(tab({ kind: 'settings', section: 'appearance' }, leaf(host.id), [host]))

    // The two round trips an import makes, answered as Rust does: the new schemes ride their own
    // event (they are not in the workspace), and the picker moving to one is a settings write
    // that comes back as a workspace snapshot.
    handlers.set('scheme_import', (): ColorScheme[] => {
      world.boot.schemes = [...world.boot.schemes, ...structuredClone(IMPORTED)]
      emitEvent('cide://schemes-changed', { schemes: world.boot.schemes })
      return IMPORTED
    })
    // A fresh workspace object per write, never an in-place edit: the store holds the very
    // object `app_get_bootstrap` answered, so bumping `rev` on it would make the snapshot look
    // "not newer" than itself and the store would drop it.
    handlers.set('settings_set', (a: Args): Settings => {
      const ws = world.boot.workspace
      const next = { ...ws, rev: wire(Number(ws.rev) + 1), settings: { ...ws.settings, ...(a['patch'] as SettingsPatch) } }
      world.boot.workspace = next
      emitEvent('cide://workspace-changed', { rev: Number(next.rev), workspace: structuredClone(next) })
      return next.settings
    })
    handlers.set('graphics_status', (): GraphicsStatus => GRAPHICS)
    handlers.set('agents_models', (): AgentModels => ({ harness: 'opencode', models: [], problem: null }))
    // The seed is `cide-headless`'s, and says so; the About line should name the app.
    world.boot.capabilities.version = '0.9.1'
  },
  drive: async () => {
    const importButton = () => [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Import…')
    await until(() => importButton() !== undefined)
    importButton()?.click()
    await sleep(500)
    await click('[role="combobox"][aria-label="Colour scheme"]')
  },
}
