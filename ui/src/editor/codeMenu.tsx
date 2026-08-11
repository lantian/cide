/**
 * The code pane's context menu.
 *
 * Right-clicking a buffer used to produce nothing at all: `menus/native.ts` suppresses
 * WebKitGTK's own menu everywhere that is not a text input, and a CodeMirror surface is not a
 * text input as far as `wantsNativeMenu` can tell (it is a `contenteditable`, but the editor's
 * host opts the subtree out). So the whole clipboard was unreachable by mouse in the one pane
 * of the app that is mostly text.
 *
 * # What is here and what is deliberately not
 *
 * Cut / Copy / Paste, Select all, Find, Go to definition, and *@-mention the selection*.
 *
 * **Go to definition is drawn and disabled**, with the reason spelled out. That is a
 * deliberate use of `disabledReason` rather than an omission: it is the item people go looking
 * for in a code pane's menu, and "there is no language server yet" is a much better answer
 * than a menu that appears not to have the feature at all. The moment an LSP exists this line
 * gets a `run` and nothing else changes.
 *
 * **Paste is the reason this file needed a plugin.** See `clipboard.ts`.
 *
 * # Why the commands are CodeMirror's rather than the app's keymap
 *
 * `selectAll` and `openSearchPanel` are dispatched into the view directly. Routing them
 * through the app's command layer would need two new command ids, two default bindings and a
 * `when` clause for "an editor is focused" — and the editor already has all three from
 * `defaultKeymap` and `searchKeymap`. The menu items therefore carry no `command`, so they
 * show no shortcut chip: a chip must come from the live keymap or not at all, and CodeMirror's
 * internal bindings are not in it.
 *
 * *Send lines to Claude* used to be the exception — it carried `command: 'claude.mention.file'`
 * and drew that command's chip. It no longer does, and the reason is worth reading before
 * putting it back: **that command is registered and dispatched by nobody.** `App.tsx`'s
 * `runCommand` has no case for it and falls through to `diag.log('command not handled by this
 * window')`, and `cide-core::commands` gates it `.when("claudePaneFocused")` — which is exactly
 * inverted for a gesture whose whole premise is that an *editor* is focused. A chip advertising
 * a shortcut for a dead command is a worse lie than no chip. See `useSendToClaude.ts`, which
 * binds `Alt-Enter` inside the editor so the keyboard route works without either fix.
 */
import { selectAll } from '@codemirror/commands'
import { openSearchPanel } from '@codemirror/search'
import type { EditorView } from '@codemirror/view'
import { useContextMenu, type ContextMenuHandle, type MenuEntry } from '@/menus'
import { copyUnavailable, pasteUnavailable, readClipboard, writeClipboard } from './clipboard'
import { sendLabel } from './sendToClaude'
import { useSendToClaude } from './useSendToClaude'

export interface CodeMenuOptions {
  /** Live view, or `null` before it is built and after it is destroyed. */
  view: () => EditorView | null
  /** Absolute path of the buffer, for the mention. */
  path: string
  /** Read-only buffers offer Copy but not Cut or Paste. */
  readOnly: boolean
}

/** The selected text, and where it is. Empty `text` means the caret is just sitting somewhere. */
function selection(view: EditorView): { text: string; from: number; to: number } {
  const { from, to } = view.state.selection.main
  return { text: view.state.sliceDoc(from, to), from, to }
}

/**
 * Replace the selection with `text` and put the caret after it.
 *
 * Through a transaction rather than `execCommand('insertText')`: a transaction goes into the
 * undo history as one step and is what the editor's own dirty tracking watches. `userEvent`
 * is what makes `historyKeymap`'s undo treat this as an input rather than as an anonymous
 * change, so Ctrl+Z after a paste removes the paste and not the three characters typed before
 * it.
 */
function insert(view: EditorView, text: string): void {
  const { from, to } = view.state.selection.main
  view.dispatch({
    changes: { from, to, insert: text },
    selection: { anchor: from + text.length },
    scrollIntoView: true,
    userEvent: 'input.paste',
  })
  view.focus()
}

export function useCodeMenu({ view, path, readOnly }: CodeMenuOptions): ContextMenuHandle {
  const toClaude = useSendToClaude()

  return useContextMenu({
    label: 'Code',
    items: (): readonly MenuEntry[] => {
      const live = view()
      if (live === null) return []
      const sel = selection(live)
      const hasSelection = sel.text.length > 0

      /*
       * One sentence, reused. "Select some text first" is the honest reason for four items and
       * writing it four times is how three of them end up saying something slightly different.
       */
      const needsSelection = hasSelection ? undefined : 'Select some text first'

      return [
        {
          id: 'cut',
          label: 'Cut',
          disabledReason: readOnly
            ? 'This buffer is read-only'
            : (needsSelection ?? copyUnavailable() ?? undefined),
          run: () => {
            void writeClipboard(sel.text).then((ok) => {
              // The delete happens only if the write landed. A Cut that copied nothing and
              // deleted anyway is data loss with no undo the user knows to reach for.
              if (!ok) return
              live.dispatch({
                changes: { from: sel.from, to: sel.to, insert: '' },
                userEvent: 'delete.cut',
              })
              live.focus()
            })
          },
        },
        {
          id: 'copy',
          label: 'Copy',
          disabledReason: needsSelection ?? copyUnavailable() ?? undefined,
          run: () => {
            void writeClipboard(sel.text)
            live.focus()
          },
        },
        {
          id: 'paste',
          label: 'Paste',
          // No `command`. `terminal.paste` is the nearest app command and it is the *wrong*
          // one — its chip is the terminal's binding, and this pastes with the webview's own
          // Ctrl+V, which the keymap does not own. A chip must come from the live keymap or
          // not at all.
          disabledReason: readOnly ? 'This buffer is read-only' : (pasteUnavailable() ?? undefined),
          run: () => {
            void readClipboard().then((text) => {
              if (text === null || text.length === 0) return
              insert(live, text)
            })
          },
        },
        { kind: 'separator' },
        {
          id: 'selectAll',
          label: 'Select all',
          run: () => {
            selectAll(live)
            live.focus()
          },
        },
        {
          id: 'find',
          label: 'Find…',
          run: () => {
            openSearchPanel(live)
          },
        },
        { kind: 'separator' },
        {
          id: 'goToDefinition',
          label: 'Go to definition',
          // Present on purpose. See the module comment: the absence of an LSP is a better
          // answer than the absence of the line.
          disabledReason: 'No language server yet — cide does not index symbols',
        },
        {
          id: 'mention',
          label: sendLabel(toClaude.range(live)),
          /*
           * **No `command`, and that is a report rather than a preference.**
           *
           * `claude.mention.file` is in the registry, so it draws a chip — and running it does
           * nothing: `App.tsx`'s dispatcher has no case for it and falls through to
           * `diag.log('command not handled by this window')`. It is also gated
           * `.when("claudePaneFocused")`, which is exactly backwards for a gesture made from an
           * editor. A chip pointing at a dead command is worse than no chip, so it is gone
           * until those two lines land — one in `App.tsx`, one in `cide-core/src/commands.rs`,
           * neither of them this change's file. Meanwhile the item and its `Alt-Enter` binding
           * both work without either.
           */
          disabledReason: toClaude.unavailable ?? undefined,
          run:
            toClaude.unavailable !== null
              ? undefined
              : () => {
                  toClaude.send(live, path)
                },
        },
      ]
    },
  })
}
