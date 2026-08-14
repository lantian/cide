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
 * **Go to definition is live** as of M12's second half, and the promise this header used to make
 * — "the moment an LSP exists this line gets a `run` and nothing else changes" — is what came due.
 * It was drawn-and-disabled for two milestones on purpose: it is the item people go looking for in
 * a code pane's menu, and "there is no language server yet" is a much better answer than a menu
 * that appears not to have the feature at all. It now resolves through the server rather than
 * guessing from the symbol index; the item's own comment says why that distinction is the whole
 * feature. It is still disabled — with a different sentence — for a pane that is not in a
 * project, because there is nothing to resolve against.
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
import { levelFor, setLevel } from './highlightLevel'
import { goToDefinition } from './goToDefinition'
import { sendLabel } from './sendToClaude'
import { useSendToClaude } from './useSendToClaude'

export interface CodeMenuOptions {
  /** Live view, or `null` before it is built and after it is destroyed. */
  view: () => EditorView | null
  /** Absolute path of the buffer, for the mention. */
  path: string
  /**
   * The workspace default the highlighting items tick against when this file has no override.
   *
   * Passed in rather than read from settings here, so this module keeps taking everything it
   * needs as arguments — the same reason it takes `path` rather than reaching for the focused
   * tab.
   */
  defaultLevel?: 'none' | 'syntax' | 'all' | undefined
  /** Read-only buffers offer Copy but not Cut or Paste. */
  readOnly: boolean
  /**
   * The project this buffer belongs to, for Go to definition.
   *
   * Threaded from `EditorPane` rather than read from `useMentionTarget()`, and the difference is
   * not cosmetic: that hook derives a project from the window's *role*, which is where a mention
   * should go, not which project owns this file. For a buffer belonging to another project the
   * two differ, and the jump would open the target in the wrong project's tab list.
   *
   * Absent means "this pane is not in a project", which is a real state — the item stays disabled
   * and says so rather than guessing.
   */
  project?: string | undefined
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

export function useCodeMenu({
  view,
  path,
  readOnly,
  defaultLevel = 'all',
  project,
}: CodeMenuOptions): ContextMenuHandle {
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
          command: 'navigate.definition',
          /*
           * Live since M12's second half, and the file header's promise — "the moment an LSP
           * exists this line gets a `run`" — is what came due here.
           *
           * It resolves through the language server's `textDocument/definition` and nothing else.
           * cide *indexes* declarations (that is what Ctrl+Alt+Shift+N searches) and deliberately
           * does not use that index for this: jumping to whichever of the eleven `fn new` in a
           * workspace shares the identifier's spelling is a wrong answer dressed as a right one.
           * When no server is running, `goToDefinition` reports the server's own sentence rather
           * than guessing.
           *
           * `disabledReason` must **not** be set alongside `run`. `menus/model.ts` resolves
           * `run: enabled ? (entry.run ?? null) : null`, so an item carrying both looks wired and
           * is dead — the exact silent-inertness this project keeps re-shipping.
           */
          disabledReason:
            project === undefined
              ? 'This editor is not part of a project, so there is nothing to resolve against'
              : undefined,
          run:
            project === undefined
              ? undefined
              : () => {
                  const { from } = live.state.selection.main
                  const line = live.state.doc.lineAt(from)
                  // 1-based line, and a 1-based UTF-16 column — `from - line.from` is already a
                  // UTF-16 offset because that is what a CodeMirror document position is.
                  goToDefinition(project, path, line.number, from - line.from + 1)
                },
        },
        { kind: 'separator' },
        /*
         * IDEA's highlighting-level widget, as three checked items.
         *
         * Not a submenu: `MenuItem` has none, and adding submenu support to the whole menu system
         * for a three-way reading mode is a larger change than the mode is worth. Not the status
         * bar either — that is window-global and this is per editor, and a control whose scope
         * disagrees with its position is how a user turns highlighting off in one file and
         * concludes their language server broke.
         */
        ...(['all', 'syntax', 'none'] as const).map((value) => ({
          id: `highlight:${value}`,
          label:
            value === 'all'
              ? 'Highlighting: all problems'
              : value === 'syntax'
                ? 'Highlighting: syntax only'
                : 'Highlighting: none',
          checked: levelFor(path, defaultLevel) === value,
          run: () => setLevel(path, value),
        })),
        {
          id: 'mention',
          label: sendLabel(toClaude.range(live)),
          /*
           * **Still no `command`, and the reason has changed — which is why the old one is
           * written out rather than deleted.**
           *
           * It used to be that `claude.mention.file` was dispatched by nothing and gated
           * `.when("claudePaneFocused")`, so a chip here would have pointed at a dead command
           * that the palette also hid from the only surface it is used on. Both of those were
           * fixed elsewhere: the arm exists in `keys/dispatch.ts` and the clause is
           * `editorFocused && claudeTarget`.
           *
           * What has not changed is that they are not the same act. The registry command
           * mentions the *focused file*, whole — it is dispatched with no editor in hand and
           * cannot see a selection. This item mentions the range under the caret. Wiring the
           * chip on would advertise a shortcut that quietly drops the user's selection, which
           * is a worse lie than no chip. `Alt-Enter` in the buffer is the keyboard half of
           * *this* item and is bound in `EditorSurface`.
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
