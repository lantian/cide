/**
 * Command dispatch — what actually happens when the gate resolves a chord, or the palette
 * runs a row.
 *
 * One dispatcher for both, deliberately. A key and a palette entry that name the same
 * command must do the same thing; two switch statements is how they stop doing so, and the
 * palette is where the difference would be least visible because a user who runs a command
 * from a list rarely also has the key memorised.
 *
 * This layer owns the commands whose effect lives in M8's surfaces — the overlays and the
 * sidebar — and the two file-save commands, which belong here because the thing that can
 * honour them is the open-buffer registry and not the workspace. Everything else is forwarded
 * to `fallback`, which is the app's existing workspace dispatcher. An unknown command is
 * logged rather than dropped: a binding whose command id is a typo is otherwise silent, which
 * is the failure mode `cide-core::commands` documents at length.
 *
 * Nothing here takes its state from a prop. Each store is read with `getState()` at the
 * moment the command runs, because the key gate is a window listener living outside React —
 * a dispatcher closed over render output would act on whatever the last render saw.
 */
import { useOverlays } from '@/overlays/store'
import { useFileTree } from '@/sidebar/treeStore'
import { useWorkspace } from '@/store/workspace'
import { diag } from '@/ipc/client'
import { focusedTabOf, registeredBuffers, saveAll, saveTab } from '@/editor/openBuffers'

export interface DispatchDeps {
  /** Commands this layer does not own: pane splits, tabs, git, theme, settings. */
  fallback: (command: string, args: unknown) => void
  /** Switch the activity rail's sidebar view, for `sidebar.files` / `sidebar.git`. */
  showSidebar?: ((view: 'files' | 'git') => void) | undefined
  /**
   * Override for which tab `file.save` writes.
   *
   * Nothing supplies it, and nothing needs to: the default reads the same workspace mirror
   * the app renders from, so `file.save` works in a window that passes only a `fallback`. It
   * stays on the interface for one reason — a host that *does* pass it (this field shipped
   * before the default existed, with an `App.tsx` line proposed to fill it) keeps
   * typechecking, and passing `() => focused?.tab.id ?? null` is the same answer.
   *
   * It is not the place to add "save the pane the caret is in". A File tab has exactly one
   * editor by construction — `PaneBody` dispatches on the pane kind for that reason — so the
   * tab is the whole address of a buffer.
   */
  focusedTab?: (() => string | null) | undefined
}

/**
 * The focused tab of the live workspace mirror.
 *
 * `getState()` rather than a hook, for the same reason the overlay and file-tree stores above
 * are read that way: the key gate is a window listener that runs outside React, so a
 * dispatcher built from hook output would close over whatever the last render happened to
 * see. The mirror being a zustand store is exactly what makes this reachable from here — it
 * was the reason `file.save` was believed to need wiring from `App.tsx`, and it is the reason
 * it does not.
 *
 * The arithmetic is in [`focusedTabOf`], in `editor/openBuffers.ts`, because that module can
 * be compiled and driven from fixtures and this one cannot. Passing `boot` — a `Bootstrap` —
 * to a parameter typed structurally is what keeps that mirror honest: a renamed field or a
 * new `WindowRole` variant fails to typecheck here.
 */
function focusedTabOfWorkspace(): string | null {
  return focusedTabOf(useWorkspace.getState().boot)
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
       * The tab comes from the workspace mirror by default and not from a `DispatchDeps`
       * field the host must remember to pass. Forwarding to `fallback` when the host passed
       * nothing was the shape this had first, and it is a dead command wearing a diagnostic:
       * it turns "Ctrl+S does not save" into "Ctrl+S does not save and says so in a log
       * nobody reads". `useWorkspace` is a module-level store read with `getState()` — the
       * same way this function already reaches the overlay and file-tree stores — so there
       * was never anything for the host to supply.
       *
       * Saving nothing is still an ordinary outcome and still says nothing: Ctrl+S with a
       * Claude tab focused finds no editor registered for that tab, and a diagnostic line per
       * keystroke for a keystroke that did the right thing is noise.
       *
       * The stroke stays swallowed either way. Letting it through to a focused terminal would
       * send `^S`, which is flow control, and freeze the pane with no visible cause.
       */
      case 'file.save': {
        const saving = saveTab((deps.focusedTab ?? focusedTabOfWorkspace)())
        // Failures are reported by the pane that owns the file, and the tab stays dirty —
        // which is what puts the close confirmation in front of the user. Swallowed here so
        // an unhandled rejection does not reach the window.
        if (saving !== null) void saving.catch(() => {})
        return
      }

      /*
       * Every live editor, not every dirty one — and that is a compromise, not a design.
       *
       * `openBuffers` holds savers, not dirty flags: the flag lives in `EditorSurface`'s
       * `baseline`, travels *outward* through `tab_set_dirty`, and never comes back — Rust
       * has no command that answers "which tabs are unsaved". So the only list this layer can
       * produce is "every editor that is mounted".
       *
       * For a buffer that matches its file the extra write costs an mtime and nothing else.
       * It is **not** free for a buffer that is clean and *stale*: there is no fs watcher yet
       * — `EditorPane` follows `cide://session-tool` only — so a file rewritten by `sed -i`
       * in a shell pane is still showing its old contents, and *Save all files* writes those
       * old contents back over it. That is a narrow window and it is a real one, and the way
       * to close it is a `dirty` predicate on the registry entry, which needs `EditorPane` to
       * pass one. Saving only the tabs Rust reports as unsaved is the other shape, and it
       * needs a command that does not exist (`app_quit_requested` is the only thing that
       * answers today, and asking it here would be a quit).
       */
      case 'file.saveAll': {
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
