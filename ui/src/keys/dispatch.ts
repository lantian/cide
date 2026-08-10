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
import { registeredBuffers, saveAll, saveTab } from '@/editor/openBuffers'

export interface DispatchDeps {
  /** Commands this layer does not own: pane splits, tabs, git, theme, settings. */
  fallback: (command: string, args: unknown) => void
  /** Switch the activity rail's sidebar view, for `sidebar.files` / `sidebar.git`. */
  showSidebar?: ((view: 'files' | 'git') => void) | undefined
  /**
   * The workspace tab that currently has focus, for `file.save`.
   *
   * The buffer lives in a CodeMirror state inside the pane and is reachable only through the
   * saver `editor/openBuffers.ts` holds for its tab, so saving needs a tab id and this layer
   * has no way to learn one — the key gate runs outside React and the palette is a list of
   * strings. The app supplies it.
   *
   * Optional, and `file.save` degrades rather than misfires without it: see the case below.
   */
  focusedTab?: (() => string | null) | undefined
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

      /*
       * Ctrl+S, and the palette's *Save file*.
       *
       * This has to be here rather than left to CodeMirror. `ctrl+s` is in
       * `cide_core::keymap::defaults()`, so the gate resolves it, swallows the keystroke and
       * calls this dispatcher — CodeMirror's own `Mod-s` binding in `EditorSurface` never
       * sees the event, and before this case existed the keystroke reached the fallback and
       * was logged. A key bound to a command nothing dispatches is worse than an unbound one:
       * the editor's own handler would at least have worked.
       *
       * Unbinding `ctrl+s` instead was the alternative, and it loses on the palette. *Save
       * file* is a registry entry the palette lists whether or not a key is bound to it, and
       * a row that logs a diagnostic is the same dead command in a different surface.
       *
       * The two ways this can save nothing are deliberately not the same. A host that never
       * supplied `focusedTab` can never save anything, which is a wiring mistake and goes to
       * `fallback` to be logged as unhandled. A host that supplied one whose tab has no
       * editor is the ordinary outcome of Ctrl+S with a terminal focused, and says nothing —
       * a diagnostic line per keystroke for a keystroke that did the right thing is noise.
       *
       * Either way the stroke stays swallowed. Letting it through to a focused terminal would
       * send `^S`, which is flow control, and freeze the pane with no visible cause.
       */
      case 'file.save': {
        if (deps.focusedTab === undefined) {
          deps.fallback(command, args)
          return
        }
        const saving = saveTab(deps.focusedTab())
        // Failures are reported by the pane that owns the file, and the tab stays dirty —
        // which is what puts the close confirmation in front of the user. Swallowed here so
        // an unhandled rejection does not reach the window.
        if (saving !== null) void saving.catch(() => {})
        return
      }

      case 'file.saveAll': {
        // Every live editor, not every dirty one: `openBuffers` holds savers, not dirty
        // flags, and a save of an unmodified buffer writes bytes identical to the file's.
        // Asking the panes which are dirty would mean lifting that state out of them for no
        // gain the user can see.
        const tabs = registeredBuffers()
        if (tabs.length === 0) return
        void saveAll(tabs).then(({ failed }) => {
          if (failed.length > 0) void diag.log(`file.saveAll: ${failed.length} file(s) not saved`)
        })
        return
      }

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
