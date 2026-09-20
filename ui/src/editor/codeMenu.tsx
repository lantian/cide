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
 * selection* — which is the one row here that is not a row: it opens a submenu of the project's
 * Claude conversations, so the mention can be aimed at one by name. See the item itself.
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
 * and drew that command's chip. It no longer does. The original reason has since been repaired
 * elsewhere (that command was dispatched by nobody and gated `claudePaneFocused`; the arm now
 * exists in `keys/dispatch.ts` and the clause is `editorFocused && claudeTarget`), and the
 * reason it still carries no chip is a different one, written out on the item: the command
 * mentions the focused file whole at a destination the app picks, and this row opens a list of
 * conversations for the range under the caret. Two different acts. `Alt-Enter` in the buffer is
 * bound in `EditorSurface` and is the keyboard route to the *automatic* destination.
 */
import { redo, redoDepth, selectAll, undo, undoDepth } from '@codemirror/commands'
import { openSearchPanel } from '@codemirror/search'
import type { EditorView } from '@codemirror/view'
import { useContextMenu, type ContextMenuHandle, type MenuEntry } from '@/menus'
import { copyUnavailable, pasteUnavailable, readClipboard, writeClipboard } from './clipboard'
// The store's own toggle, not a second implementation of it. It owns the fetch, the 2 MiB cap,
// the dirty-buffer contents and the latest-wins slot; a menu item that re-derived any of that
// would be a second answer to "is this file annotated" for the same buffer.
import { toggleBlame } from './blameStore'
import { levelFor, setLevel } from './highlightLevel'
import { goToDefinition } from './goToDefinition'
import { showDocumentation } from './quickDocumentation'
import { focusedFormat } from './caretTrack'
import { formatDocument } from './formatDocument'
import { findUsages } from './codeIntel'
import { wordTargetAt } from './ctrlLink'
import {
  canFold,
  canFoldAll,
  canUnfold,
  canUnfoldAll,
  foldAllRanges,
  foldHere,
  foldRecursive,
  unfoldAllRanges,
  unfoldHere,
  unfoldRecursive,
} from './folding'
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
  /**
   * Whether the blame gutter is currently on for this buffer. (M18)
   *
   * A **fact**, not a label: the item below is a `checked` toggle, so it reads *Annotate with git
   * blame* both ways and the tick says which. The alternative — renaming the item to *Remove
   * annotations* when it is on — was rejected for the reason IDEA's own Annotate is a checkbox: a
   * menu whose items rename themselves between openings is one the user has to re-read every
   * time, and the two names would be one more pair of strings to keep in step.
   *
   * Passed in rather than read from `blameStore` here, so this module goes on taking everything
   * it needs as arguments — the same rule `path`, `project` and `defaultLevel` already follow.
   * Defaults to `false`, so a host that has not wired it yet draws an unticked toggle rather
   * than failing to compile.
   */
  blameOn?: boolean | undefined
  /**
   * *Show history for this file* — the tool window's per-file tab. (M18)
   *
   * A callback and not a direct call, because this module cannot reach the command layer: the
   * dispatcher is built in `App.tsx` and there is no global instance to look one up in (that is
   * deliberate — see `keys/dispatch.ts::createDispatcher`). The host wires it to
   * `runCommand('git.history.file', { path })` and to nothing else, which is the `file.reveal`
   * precedent: one handler owns "is this a shell window, does the project hold a repository,
   * which repository is this path in", and the tab strip, the file tree and the git panel all
   * reach the same one.
   *
   * Reaching for `history.locate` + `toolWindow.openHistory` here instead would be a fourth copy
   * of those preconditions in the one surface that cannot see whether the window even has a tool
   * window.
   *
   * Optional: without it the item is drawn disabled with the reason on it, which is the honest
   * shape for a detached-pane window.
   */
  onShowHistory?: ((path: string) => void) | undefined
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
  blameOn = false,
  onShowHistory,
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
       * Ask Rust for the `/rename` names now, while the *parent* menu is being built.
       *
       * The submenu that shows them is built later — `MenuItem.submenu` is a thunk the menu
       * calls when the row is hovered — so this round trip has the whole of that gap to land
       * in, which for a human moving a pointer is several hundred milliseconds against one
       * IPC call and a read of eight small files. Fire and forget: a refresh that fails costs
       * the submenu its labels and nothing else, and `claudeNames.ts` says why that is a log
       * line rather than a toast.
       *
       * Doing it here rather than in an effect is deliberate. An effect would have to fire on
       * something, and the only honest trigger for "a name the user typed into another program
       * changed" is "somebody is about to look at it" — which is this, exactly.
       */
      toClaude.refresh()

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
        /*
         * Reformat code. (M26)
         *
         * A flat row rather than a submenu, and above *Folding ▸* because it acts on the file
         * where everything below it acts on the view.
         *
         * **No `disabledReason`.** Whether anything can format this buffer is a question only
         * Rust can answer — it depends on which language servers are running, what each of them
         * advertised at its handshake, and what the user configured — and asking it to paint a
         * menu would mean an IPC round trip every time the menu opened. So the row is always
         * live and the refusal is a sentence after the fact, which is the same trade
         * *Go to definition* below makes for the same reason. Note `menus/model.ts` resolves
         * `run: enabled ? (entry.run ?? null) : null`, so a `disabledReason` set *beside* a
         * `run` yields a row that looks wired and is dead — never both.
         *
         * `command` names the id so the chip on the right comes from the live keymap rather than
         * a literal a `keymap.json` could silently falsify.
         */
        {
          id: 'format',
          label: 'Reformat code',
          command: 'editor.format',
          disabledReason:
            project === undefined
              ? 'This editor is not part of a project, so there is no formatter to run'
              : undefined,
          run:
            project === undefined
              ? undefined
              : () => {
                  /*
                   * `focusedFormat()` and not a `FormatActions` assembled here, deliberately.
                   * The one in `EditorSurface` is where the `EditorView` lives, and a second
                   * implementation would be a second copy of the staleness re-check and the
                   * minimal-change trim — the copy that is wrong when one of them changes.
                   *
                   * `live.focus()` first, because that is what makes the slot answer *this*
                   * editor: opening a context menu moved focus off the buffer, and in a split
                   * showing one file twice the wrong half would otherwise be formatted.
                   */
                  live.focus()
                  const actions = focusedFormat()
                  if (actions !== null) void formatDocument(project, actions)
                },
        },
        /*
         * Folding, as a submenu. (M19)
         *
         * IDEA files these under *Folding ▸* and so does this, for the reason the submenu exists
         * at all: six rows is more than a third of this menu, and every one of them is a gesture
         * a keyboard user has already learnt a chord for. Flattening them would push *Go to
         * definition* and *Find usages* — the two rows people actually open this menu for — below
         * the fold, which is the ordering complaint M14 already filed about this menu once.
         *
         * Each row `run`s the same function `keys/dispatch.ts` reaches through
         * `focusedFolds()`, and names its `command` so the chip on the right comes from the live
         * keymap rather than from a literal that a `keymap.json` would silently falsify.
         *
         * The `disabledReason`s are the point rather than polish. A menu row that looks live and
         * does nothing is the state `menus/model.ts` refuses to represent, and every one of these
         * six is inapplicable most of the time — *Expand all* in a file with nothing collapsed is
         * the common case, not the edge one.
         */
        {
          id: 'folding',
          label: 'Folding',
          submenu: () => {
            const state = live.state
            return [
              {
                id: 'fold',
                label: 'Collapse',
                command: 'editor.fold',
                disabledReason: canFold(state) ? undefined : 'Nothing to collapse at the caret',
                run: () => {
                  foldHere(live)
                  live.focus()
                },
              },
              {
                id: 'unfold',
                label: 'Expand',
                command: 'editor.unfold',
                disabledReason: canUnfold(state) ? undefined : 'Nothing is collapsed at the caret',
                run: () => {
                  unfoldHere(live)
                  live.focus()
                },
              },
              { kind: 'separator' },
              {
                id: 'foldRecursively',
                label: 'Collapse recursively',
                command: 'editor.foldRecursively',
                disabledReason: canFold(state) ? undefined : 'Nothing to collapse at the caret',
                run: () => {
                  foldRecursive(live)
                  live.focus()
                },
              },
              {
                id: 'unfoldRecursively',
                label: 'Expand recursively',
                command: 'editor.unfoldRecursively',
                disabledReason: canUnfold(state) ? undefined : 'Nothing is collapsed at the caret',
                run: () => {
                  unfoldRecursive(live)
                  live.focus()
                },
              },
              { kind: 'separator' },
              {
                id: 'foldAll',
                label: 'Collapse all',
                command: 'editor.foldAll',
                disabledReason: canFoldAll(state)
                  ? undefined
                  : 'This file has nothing left to collapse',
                run: () => {
                  foldAllRanges(live)
                  live.focus()
                },
              },
              {
                id: 'unfoldAll',
                label: 'Expand all',
                command: 'editor.unfoldAll',
                disabledReason: canUnfoldAll(state)
                  ? undefined
                  : 'Nothing in this file is collapsed',
                run: () => {
                  unfoldAllRanges(live)
                  live.focus()
                },
              },
            ]
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
                  goToDefinition(
                    project,
                    path,
                    line.number,
                    from - line.from + 1,
                    undefined,
                    wordTargetAt(live, from)?.text ?? '',
                  )
                },
        },
        {
          id: 'quickDocumentation',
          label: 'Quick documentation',
          command: 'navigate.documentation',
          /*
           * A page about the symbol under the caret, from whichever server owns the file. (M60)
           * The same `disabledReason` XOR `run` rule as the item above: an item carrying both
           * looks wired and is dead.
           */
          disabledReason:
            project === undefined
              ? 'This editor is not part of a project, so there is nothing to ask'
              : undefined,
          run:
            project === undefined
              ? undefined
              : () => {
                  const { from } = live.state.selection.main
                  const line = live.state.doc.lineAt(from)
                  showDocumentation(
                    project,
                    path,
                    line.number,
                    from - line.from + 1,
                    wordTargetAt(live, from)?.text ?? '',
                  )
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
         * The two git questions about *this file*, as their own group between the two symbol
         * questions above and the reading-mode ones below. (M18)
         *
         * They belong together and away from both neighbours: `Go to definition` and
         * `Find usages` are about the symbol under the caret, these two are about the buffer as
         * a whole, and the highlighting items are about how it is painted. IDEA groups them the
         * same way, and it is the grouping that makes a fourteen-item menu readable at all.
         */
        {
          id: 'blame',
          label: 'Annotate with git blame',
          command: 'git.blame',
          /*
           * A **toggle**, not a pair of items that rename themselves. `checked` puts a tick in
           * the gutter `menus/model.ts` reserves for exactly this, so the row reads the same
           * both ways and the state is a glance rather than a re-read. See the `blameOn` option.
           *
           * Routed to `toggleBlame` directly rather than to the command, and this is the one
           * new item in M18 that does **not** go through `runCommand` — deliberately, and the
           * reason is that there is nothing for a command to add here. `git.blame`'s dispatch
           * arm is three lines: resolve the focused file's path, resolve the active project,
           * call `toggleBlame`. This menu already has both in hand — `path` is the buffer's own
           * and is *better* than the focused-tab guess the command has to make — so routing
           * through the registry would replace two known values with two re-derived ones. The
           * command id is still named above, so the row draws whatever chord the keymap has for
           * it (nothing today: the gate is a window *capture* listener, so every chord it claims
           * is taken from every terminal pane in every window, and IDEA ships Annotate unbound
           * too).
           *
           * `disabledReason` must **not** be set alongside `run` — `menus/model.ts` resolves
           * `run: enabled ? (entry.run ?? null) : null`, so an item carrying both looks wired
           * and is dead. The two items above say the same thing; this is the third, and it is
           * repeated rather than referenced because the failure is silent and the fix is local.
           */
          checked: blameOn,
          disabledReason:
            project === undefined
              ? 'This editor is not part of a project, so there is no repository to annotate from'
              : undefined,
          run:
            project === undefined
              ? undefined
              : () => {
                  // `project` is a `ProjectId`; the option is typed `string` for the reason
                  // `goToDefinition`'s call two items up takes one — see that option's note.
                  toggleBlame(project, path)
                },
        },
        {
          id: 'history',
          label: 'Show history for this file',
          command: 'git.history.file',
          /*
           * The same act as the palette row and as the tab strip's item, so it carries the
           * command id and draws whatever chord the live keymap has for it.
           *
           * Two ways to be unavailable and two sentences. The project one is this menu's
           * standing answer for "there is nothing to resolve against"; the host one is the
           * detached-pane window, which has no tool window for the tab to open in — and that is
           * a different fact from having no project, so it gets different words.
           *
           * `disabledReason` and `run` are again mutually exclusive; see the item above.
           */
          disabledReason:
            project === undefined
              ? 'This editor is not part of a project, so there is no history to show'
              : onShowHistory === undefined
                ? 'This window has no tool window for a history tab'
                : undefined,
          run:
            project === undefined || onShowHistory === undefined
              ? undefined
              : () => {
                  onShowHistory(path)
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
           * **A submenu since M19, and the row itself no longer sends.**
           *
           * > *"this should be not just a button, but element with inner elements — when
           * > hovering I should be able to select to which claude session send this"*
           *
           * The rows are every Claude pane in the project, numbered the way the user sees them
           * and named the way the user named them — `2: git-details`, from `/rename`. See
           * `claudeSessions.ts` for the ordering and `claudeNames.ts` for where a name comes
           * from. Picking one is an **exact** send: the pane is a stated choice rather than a
           * guess, so `claude_send_lines` addresses it and refuses instead of rerouting.
           *
           * What that costs, stated because it is a real loss: the *automatic* destination —
           * focused Claude pane, else the console, rerouted by Rust to whichever can actually
           * receive — is now reachable from `Alt-Enter` in the buffer and not from this menu.
           * A parent row cannot also be a button (`MenuItem.submenu` says why: one click, two
           * meanings, and one of them unreachable by mouse), and given the choice between
           * "which conversation?" and "any conversation" on the mouse route, the user asked
           * for the first. The rows that *are* named are exactly the ones with a live `claude`
           * behind them, which is the same information the automatic route was using.
           *
           * **Still no `command`, and the reason has changed twice — which is why both are
           * written out rather than deleted.**
           *
           * It used to be that `claude.mention.file` was dispatched by nothing and gated
           * `.when("claudePaneFocused")`, so a chip here would have pointed at a dead command
           * that the palette also hid from the only surface it is used on. Both of those were
           * fixed elsewhere: the arm exists in `keys/dispatch.ts` and the clause is
           * `editorFocused && claudeTarget`.
           *
           * What has not changed is that they are not the same act. The registry command
           * mentions the *focused file*, whole, at whatever destination the app picks — it is
           * dispatched with no editor in hand and cannot see a selection, let alone a chosen
           * session. This row opens a list of conversations for the range under the caret.
           * A chip here would advertise a shortcut that drops both the selection and the
           * choice, which is a worse lie than no chip.
           */
          disabledReason: toClaude.unavailable ?? undefined,
          submenu:
            toClaude.unavailable !== null
              ? undefined
              : () =>
                  toClaude.sessions().map((session) => ({
                    // The pane id, not the position: `2` renumbers the moment a pane closes,
                    // and a React key that moves between renders is a row that loses its
                    // hover in the middle of being clicked.
                    id: `mention:${session.pane}`,
                    label: session.label,
                    run: () => {
                      toClaude.sendTo(live, path, session.pane)
                    },
                  })),
        },
      ]
    },
  })
}
