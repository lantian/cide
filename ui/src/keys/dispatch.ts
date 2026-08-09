/**
 * Command dispatch — what actually happens when the gate resolves a chord, or the palette
 * runs a row.
 *
 * One dispatcher for both, deliberately. A key and a palette entry that name the same
 * command must do the same thing; two switch statements is how they stop doing so, and the
 * palette is where the difference would be least visible because a user who runs a command
 * from a list rarely also has the key memorised.
 *
 * This layer owns only the commands whose effect lives in M8's surfaces — the overlays and
 * the sidebar. Everything else is forwarded to `fallback`, which is the app's existing
 * workspace dispatcher. An unknown command is logged rather than dropped: a binding whose
 * command id is a typo is otherwise silent, which is the failure mode `cide-core::commands`
 * documents at length.
 */
import { useOverlays } from '@/overlays/store'
import { useFileTree } from '@/sidebar/treeStore'
import { diag } from '@/ipc/client'

export interface DispatchDeps {
  /** Commands this layer does not own: pane splits, tabs, git, theme, settings. */
  fallback: (command: string, args: unknown) => void
  /** Switch the activity rail's sidebar view, for `sidebar.files` / `sidebar.git`. */
  showSidebar?: ((view: 'files' | 'git') => void) | undefined
}

/**
 * Build the dispatcher.
 *
 * Returns a plain function rather than a hook so the key gate — which runs outside React —
 * and the palette can share one instance. The stores it writes to are read with
 * `getState()` for the same reason.
 */
export function createDispatcher(deps: DispatchDeps): (command: string, args: unknown) => void {
  return (command, args) => {
    switch (command) {
      case 'picker.files':
        // `toggle`, not `show`: Ctrl+P with the picker already up closes it. The gate
        // swallows the chord either way, so a `show` here would leave the user with no
        // keystroke that dismisses what their keystroke opened except Escape.
        useOverlays.getState().toggle('files')
        return

      case 'palette.commands':
        useOverlays.getState().toggle('commands')
        return

      case 'sidebar.files':
        deps.showSidebar?.('files')
        return

      case 'sidebar.git':
        deps.showSidebar?.('git')
        return

      case 'file.reveal': {
        // `args` is opaque on the wire (`serde_json::Value`), so it is narrowed here rather
        // than trusted: a keymap.json entry can put anything in it.
        const path = typeof args === 'object' && args !== null ? (args as { path?: unknown }).path : undefined
        if (typeof path === 'string') void useFileTree.getState().reveal(path)
        return
      }

      default:
        deps.fallback(command, args)
    }
  }
}

/** A `fallback` that only reports. Useful until the workspace dispatcher exists. */
export function reportOnly(command: string, args: unknown): void {
  void diag.log(`[cide] command not handled: ${command} ${args === undefined ? '' : JSON.stringify(args)}`)
}
