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
 * Undo / Redo, Cut / Copy / Paste, Select all, Find, Go to definition, and *@-mention the
 * selection*.
 *
 * **Undo and Redo were reachable from the keyboard and from nothing else.** `history()` and
 * `historyKeymap` have been in `EditorSurface` since M9, so Ctrl+Z, Ctrl+Y and Ctrl+Shift+Z all
 * worked — but there was no command id, no palette row and no menu item, so a user who did not
 * already know the chord had no way to find out that the buffer even had a history. That is the
 * milder half of this project's recurring defect (built, and reachable from almost nothing), and
 * a menu item is the cheapest honest fix: it names the capability, and `undoDepth`/`redoDepth`
 * let it say *why* it is greyed when there is nothing to undo, which a chord cannot.
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
 * `undo`, `redo`, `selectAll` and `openSearchPanel` are dispatched into the view directly.
 * Routing them through the app's command layer would need a new command id, a default binding
 * and a `when` clause for "an editor is focused" apiece — and the editor already has all three
 * from `defaultKeymap`, `historyKeymap` and `searchKeymap`. The menu items therefore carry no
 * `command`, so they show no shortcut chip: a chip must come from the live keymap or not at all,
 * and CodeMirror's internal bindings are not in it.
 *
 * For undo specifically the app-command route is not merely redundant, it is a **regression**,
 * and the reason is written out at length in `cide-core::keymap`'s
 * `the_editor_undo_chords_are_not_bound_here`: the key gate's second entry point is a *window
 * capture* listener, so a `ctrl+z` in the Rust table would be consumed before the event reached
 * CodeMirror, the find field, the commit box or any rename input — and Ctrl+Z in a terminal is
 * SIGTSTP. A palette row for undo would additionally have to act on a view it cannot reach:
 * opening the palette moves focus off the buffer, and there is no live-`EditorView` registry to
 * look one up in. The menu has the view in hand, through the `view()` getter below, which is
 * exactly why the menu can do this and the palette cannot.
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
import { redo, redoDepth, selectAll, undo, undoDepth } from '@codemirror/commands'
import { openSearchPanel } from '@codemirror/search'
import type { EditorView } from '@codemirror/view'
import { useContextMenu, type ContextMenuHandle, type MenuEntry } from '@/menus'
import { copyUnavailable, pasteUnavailable, readClipboard, writeClipboard } from './clipboard'
import { levelFor, setLevel } from './highlightLevel'
import { goToDefinition } from './goToDefinition'
import { findUsages } from './codeIntel'
import { wordTargetAt } from './ctrlLink'
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
    /*
     * Hand the keyboard back through CodeMirror rather than through the DOM.
     *
     * This is the fix for *"right-click, hover the menu, dismiss it — and the buffer is at the
     * top"*. The hook's default is `previous.focus({ preventScroll: true })`, which is right for
     * every other surface in the app and is not enough for this one: `preventScroll` maps to
     * `SelectionRevealMode::DoNotReveal`, so it suppresses WebKit's *reveal* — but the
     * `setSelection(firstPositionInOrBeforeNode(this))` that `Element::updateFocusAppearance`
     * performs on a root editable element with no frame selection runs **before** the reveal and
     * is not suppressed by anything. The caret would still be collapsed to the top of the buffer,
     * and CodeMirror's `DOMObserver` would read that back into state — a silent caret move
     * instead of a visible scroll, which is worse.
     *
     * `EditorView.focus()` is `focusPreventScroll(contentDOM)` **plus**
     * `docView.updateSelection()`: it takes the keyboard without scrolling and then writes the
     * selection back out of CodeMirror's own state, so both halves are restored.
     *
     * Returning `false` when the view is gone matters. The menu's action may have closed the tab
     * it hung off, and `focusReturnPlan` then falls back to the plain DOM step rather than
     * leaving the window with focus on `<body>`.
     *
     * Why the editor and not the read-only diff panes: `EditorView.editable.of(false)` means
     * `.cm-content` is not a *root editable element*, so `updateFocusAppearance` never takes the
     * branch that invents a selection. A read-only buffer is the discriminating case — it does
     * not reproduce the bug, and it does not need this.
     */
    restoreFocus: () => {
      const live = view()
      if (live === null) return false
      live.focus()
      return true
    },
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
          id: 'undo',
          label: 'Undo',
          /*
           * `undoDepth` reads the `history()` `StateField`, so it answers *this* buffer and is 0
           * both when there is nothing to undo and when the field is absent entirely — the honest
           * answer in both cases, since `undo` refuses in both. Read from `live` at menu-open
           * time, like every other reason here: `items` runs when the menu opens, and a depth
           * captured at render time would grey the item against a history two edits old.
           *
           * Read-only first, and it is not redundant with the depth. A read-only buffer has a
           * depth of 0 today, but `EditorState.readOnly` is what `undo` actually checks —
           * `@codemirror/commands` refuses on that facet before looking at the field — so naming
           * it keeps the sentence true if such a buffer ever acquires history from a
           * programmatic dispatch.
           */
          disabledReason: readOnly
            ? 'This buffer is read-only'
            : undoDepth(live.state) === 0
              ? 'Nothing to undo yet'
              : undefined,
          run: () => {
            undo(live)
            // Focus, for the reason Cut and Copy do: the menu took it, and the next Ctrl+Z has
            // to reach the buffer rather than whatever the menu left behind.
            live.focus()
          },
        },
        {
          id: 'redo',
          label: 'Redo',
          disabledReason: readOnly
            ? 'This buffer is read-only'
            : redoDepth(live.state) === 0
              ? 'Nothing to redo'
              : undefined,
          run: () => {
            redo(live)
            live.focus()
          },
        },
        { kind: 'separator' },
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
        {
          id: 'findUsages',
          label: 'Find usages',
          command: 'navigate.usages',
          /*
           * The mouse route for ⌥F7, and beside Go to definition on purpose: they are the two
           * halves of what a Ctrl+click decides between, and a user who does not trust the
           * discriminator's guess needs both named in one place.
           *
           * Unconditional where the buffer is in a project, exactly like its neighbour — the
           * search runs from a reference as happily as from a declaration, so there is no
           * precondition to test beyond having something to resolve against.
           *
           * `disabledReason` must **not** be set alongside `run`, for the reason the item above
           * spells out: `menus/model.ts` resolves `run: enabled ? (entry.run ?? null) : null`, so
           * an item carrying both looks wired and is dead.
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
                  // The same normaliser the Ctrl gestures use, so the menu and the mouse cannot
                  // disagree about where the word under the caret starts. `null` when the caret is
                  // on punctuation — the popup says "the symbol" rather than refusing.
                  const word = wordTargetAt(live, from)
                  findUsages(
                    project,
                    path,
                    line.number,
                    from - line.from + 1,
                    word?.text ?? null,
                  )
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
