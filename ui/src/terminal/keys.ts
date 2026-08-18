/**
 * Chords a terminal must resolve itself: the ones xterm.js would otherwise encode wrongly,
 * and the three that are the terminal's own rather than the key gate's — Ctrl+C, Ctrl+V and
 * Ctrl+F.
 *
 * # Shift+Enter
 *
 * There is no such thing as "Shift+Enter" on a PTY. A terminal sends `\r` for Enter, and by
 * default xterm.js sends exactly `\r` whether or not Shift was held — DEC's key encoding has
 * no modifier bits for the Return key. So a TUI reading raw bytes cannot distinguish the
 * two, and M0's "working Shift+Enter" — a newline inside a Claude prompt rather than a
 * submit — is simply unreachable without an agreed-upon encoding.
 *
 * Two encodings exist and only one of them is the one Claude Code understands:
 *
 * * `CSI 13;2u` (`\x1b[13;2u`), the modifyOtherKeys / kitty-keyboard form. Correct in the
 *   abstract, but it requires the application to have *requested* that mode; sent
 *   unsolicited to a program that did not, it arrives as a visible `[13;2u` in the prompt.
 * * `ESC CR` (`\x1b\r`) — what `claude /terminal-setup` writes into VS Code's
 *   `terminal.integrated.sendKeybindingsToShell` bindings, and therefore what the Claude TUI
 *   is known to parse as "newline, do not submit". No mode negotiation, nothing to request.
 *
 * We send `ESC CR`. The plan names it, and being bit-identical to what the vendor's own
 * setup command produces is worth more here than being theoretically tidier: this is a
 * hand-shake with one specific program, not a general terminal feature.
 *
 * # Ctrl+C, Ctrl+V and Ctrl+F, and why they are decided here and not in the keymap
 *
 * None of the three is in `cide_core::keymap::defaults()`, and a Rust test keeps it that way.
 * The obvious design — bind them with `when: "terminalFocused"` — is wrong, and the reason is
 * already written down in `sidebar/FileTree.tsx`: **these chords are focus-scoped, not
 * context-scoped.** `terminalFocused` is derived from `tab.tree.focused`, a value Rust owns,
 * and it stays true while the caret is in the file tree, the search box, the commit message, a
 * rename field or a picker. A global binding is resolved by the window *capture* listener, so
 * it would `stopPropagation` the keystroke before any of those saw it — killing the file
 * tree's own Ctrl+C/X/V and the native copy and paste of every text input in the app.
 *
 * This module is reached from one place only: the composed handler in `xterm.ts`, which xterm
 * consults for a keydown delivered to *that terminal's* textarea. That location is focus-scoped
 * by construction. And it is reached only after `terminalKeyGate` has passed the stroke
 * through, which is the same contract [`terminalKeyBytes`] already has — a chord the user has
 * bound in `keymap.json` stays bound, and binding `ctrl+c` yourself still wins.
 *
 * ## The rules, and what each one is protecting
 *
 * * **Ctrl+C copies only when there is a selection.** With no selection it returns `null` and
 *   xterm sends `ETX` exactly as it always has — same bytes, same `onUserInput` side effects.
 *   Ctrl+C is the primary escape from a runaway command and from a mid-turn `claude`, so the
 *   empty-selection branch is the one that must never change. A copy writes nothing to the pty,
 *   so `onUserInput` does not fire and the selection would survive; the caller clears it, which
 *   is what makes "select, copy, interrupt" work without reaching for the mouse.
 * * **Ctrl+V pastes, in both pane kinds.** This rule used to return `null` for a Claude pane —
 *   leaving `^V` to the CLI, which binds it and reads the *system* clipboard out of a
 *   subprocess (`xclip -selection clipboard -t image/png -o … || wl-paste …`) — because that
 *   is the only path an **image** has into a prompt and cide cannot reimplement it. The premise
 *   was right and the conclusion was wrong, and the bug report is what showed the gap: the CLI
 *   installs that handler at its *main prompt* only. At a numbered-choice prompt ("1. Yes, and
 *   use auto mode / 2. … / 3. Tell Claude what to change"), or at any of its other questions,
 *   nothing is listening for the chord — so the byte was swallowed three processes down and the
 *   keystroke did nothing at all. The context menu's Paste worked the whole time, because it
 *   calls `pasteIntoTerminal` directly and never came through here.
 *
 *   Both survive now, and the trade is made in `terminal/clipboard.ts` rather than here,
 *   because it needs an answer this function cannot wait for: the clipboard is read, text is
 *   pasted with `term.paste`, and when there is no text the `^V` byte is written at the child
 *   after the fact so the CLI's own image paste runs exactly as before. That module's header
 *   carries the whole argument, including why probing `read_image` was the wrong shape.
 *
 *   In a shell pane the paste is the same fix it always was: `bash` binds `\x16` to readline's
 *   `quoted-insert`, which swallows it and takes the next keystroke literally — "nothing
 *   happens".
 * * * **Ctrl+F opens this pane's find bar**, in both kinds. Asked for as *"Need to add CTRL+F
 *   (search bar) for claude and bash panels to be able to search text"*.
 *
 *   What it costs a child is real and is the reason it is written down rather than discovered:
 *   xterm encodes plain Ctrl+F as `^F` (0x06), which is readline's `forward-char` and vim's
 *   page-forward. Those go, in a terminal pane, on purpose.
 *
 *   **It is not a `keymap.json` default, and a `when: "terminalFocused"` one would have been a
 *   bug.** That is the same argument the Ctrl+C/Ctrl+V section above makes, and here it is
 *   sharper because Ctrl+F is a chord text surfaces actually use: `terminalFocused` is derived
 *   from `tab.tree.focused`, so it stays true while the caret is in a rename field, the commit
 *   message box or the Explorer's search input — and the gate's window *capture* listener would
 *   take the keystroke from all of them, in every window whose focused pane happens to be a
 *   terminal (which, in an IDE with a pinned Claude console, is most of the time). It would also
 *   have had to fight `ui/src/editor/EditorSurface.tsx`, whose comment says in as many words
 *   that binding `ctrl+f` in `cide-core::keymap` fights a written rule: CodeMirror's `Mod-f`
 *   opens the *editor's* find bar and is reached only because this table stays silent.
 *
 *   The command is still real and still first-class — `terminal.find` is in
 *   `cide_core::commands` with a `terminalFocused` clause, so it has a palette row and a
 *   `keymap.json` line is all it takes to put it on ⌘F for a Mac. It simply ships with no
 *   default chord, exactly as `terminal.clear` and `terminal.paste` do, and the chord that does
 *   ship is focus-scoped here where it can only ever reach a terminal that has the keystroke.
 *
 * ## Why this matches on `key` where the key gate matches on `code`
 *
 * `keys/chords.ts` resolves a *binding* by physical key, because a binding is a gesture on a
 * keyboard. This rule is not a binding: it shadows the bytes xterm would otherwise produce
 * itself, and xterm decides that from `keyCode` (`Keyboard.ts`, the `ctrlKey && !shiftKey`
 * branch), which follows the character the layout produces. Matching `code` here would let the
 * two disagree on a non-QWERTY layout: the physical C key on Dvorak produces `j`, so we would
 * copy on the keystroke xterm was about to send `^J` for. Matching `key` keeps the pair exact —
 * and where a layout makes them differ anyway, the mismatch costs a copy that does not happen
 * rather than an interrupt that does not happen.
 *
 * Ctrl only, with no `meta` spelling: nothing rewrites this module for macOS the way
 * `keymap::platform_layer` rewrites the keymap. A macOS port starts here.
 *
 * # Why this is a pure function
 *
 * Keeping the decision separate from the xterm wiring is what makes it testable without a
 * DOM, and what keeps the interaction with the key gate visible in one place —
 * `createTerminal` asks the gate first and only reaches this when the gate passed the
 * keystroke through. See `xterm.ts`.
 */

/**
 * The parts of a `KeyboardEvent` a chord is resolved from.
 *
 * Structural rather than `KeyboardEvent` so a test can pass a plain object and so this
 * module needs no DOM lib types. A real `KeyboardEvent` satisfies it.
 */
export interface TerminalKeyEvent {
  /** xterm's custom key handler is called for keydown, keypress *and* keyup. */
  type: string
  key: string
  shiftKey: boolean
  ctrlKey: boolean
  altKey: boolean
  metaKey: boolean
}

/**
 * Which of the two programs a terminal pane is hosting.
 *
 * A real distinction rather than a convenience: when the clipboard holds no text, `claude` is
 * the one program that can still do something useful with a `^V` — it shells out to
 * `xclip`/`wl-paste` and pastes an image — and `bash` turns the same byte into readline's
 * `quoted-insert`, which eats the *next* keystroke. Anything that is not the Claude CLI is
 * `'shell'`: a pane running some other program is a pane where the chord is unclaimed, which is
 * the same situation `bash` is in.
 */
export type TerminalPaneKind = 'claude' | 'shell'

/** What the rule is decided from: the pane, and what is selected in it right now. */
export interface ClipboardFacts {
  /** `term.getSelection()`. The string, not the terminal — the fact, not the object. */
  selection: string
  kind: TerminalPaneKind
}

/**
 * What this keystroke means for the clipboard, or `null` for "not ours".
 *
 * `null` is the load-bearing value: it means the caller returns `true` and *xterm* handles the
 * key, which for Ctrl+C with nothing selected is the interrupt. Every branch that is not a copy
 * or a paste has to reach it.
 */
export type ClipboardAction = { kind: 'copy'; text: string } | { kind: 'paste' } | null

/**
 * ESC CR — Shift+Enter as `/terminal-setup` encodes it.
 *
 * Exported so a test can name the constant rather than repeat the escape, and so anyone
 * grepping for `\x1b\r` finds the one place it is written.
 */
export const SHIFT_ENTER = '\x1b\r'

/**
 * The bytes this keystroke should put on the PTY *instead of* whatever xterm would send, or
 * `null` to let xterm handle it normally.
 *
 * Only genuine keydowns resolve: xterm offers keypress and keyup to the same handler, and
 * synthesising on all three would send the sequence three times for one press.
 */
export function terminalKeyBytes(ev: TerminalKeyEvent): string | null {
  if (ev.type !== 'keydown') return null

  // Plain Shift+Enter only. Ctrl+Shift+Enter and friends are left alone deliberately —
  // they are unclaimed, and claiming them here would silently shadow a future keybinding
  // for chords no one has asked this module to encode.
  if (ev.key === 'Enter' && ev.shiftKey && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
    return SHIFT_ENTER
  }

  return null
}

/**
 * Exactly the modifier shape xterm's own ctrl branch requires.
 *
 * `Keyboard.ts` encodes a control character only for `ctrlKey && !shiftKey && !altKey &&
 * !metaKey`, so any other combination is a keystroke xterm was *not* going to turn into a byte
 * — and shadowing one would be this module inventing a chord rather than shadowing one. It is
 * also what leaves Ctrl+Shift+C and Ctrl+Shift+V free: they are inert in a terminal today, and
 * they stay that way until somebody asks for them.
 */
function plainCtrl(ev: TerminalKeyEvent): boolean {
  return ev.ctrlKey && !ev.shiftKey && !ev.altKey && !ev.metaKey
}

/** Whether this keystroke is the letter `name`, as the layout produced it. See the header. */
function letter(ev: TerminalKeyEvent, name: string): boolean {
  return ev.key.length === 1 && ev.key.toLowerCase() === name
}

/**
 * Whether Ctrl+C / Ctrl+V should reach the clipboard instead of the child.
 *
 * Pure, and driven by facts rather than by a `Terminal`, so `check-terminal-keys.mjs` can run
 * every branch — including the one that must never change, which is that an empty selection
 * yields `null` and the interrupt goes through untouched.
 */
export function terminalClipboardAction(
  ev: TerminalKeyEvent,
  facts: ClipboardFacts,
): ClipboardAction {
  // keydown only. xterm offers keypress and keyup to the same handler, and a copy on all
  // three would write the clipboard three times for one press — and, worse, would return
  // `false` for the keyup of an interrupt.
  if (ev.type !== 'keydown') return null
  if (!plainCtrl(ev)) return null

  if (letter(ev, 'c')) {
    // The interrupt, whenever nothing is selected. This is the branch the feature is allowed
    // to cost nothing: `null` means xterm sends `ETX` with its own side effects (scroll to
    // bottom, clear selection), which is what a mid-turn `claude` and a runaway command are
    // stopped with. Re-implementing the interrupt as a write from a handler was the
    // alternative and it loses twice over: it changes the one keystroke that must never
    // regress, and it sends nothing at all in a pane that has no session id yet.
    //
    // `.trim()`, not `=== ''`, and the difference is the whole safety of this branch. xterm's
    // `getSelection()` is coordinate-derived and never consults `hasSelection()`, so a
    // one-pixel drag across a row boundary while clicking to focus the pane returns `"\n"`,
    // and a double-click that misses a word by one cell returns the run of spaces beside it.
    // Both are selections the user never meant to make, and against `=== ''` both turn the
    // next Ctrl+C — the one aimed at a mid-turn `claude` — into a silent clipboard write.
    //
    // The trade is deliberate and one-sided: the cost of trimming is that a deliberate
    // selection of nothing but whitespace copies nothing, which no one does on purpose; the
    // cost of not trimming is an interrupt that does not happen, which is the one keystroke
    // in this app that must never regress.
    return facts.selection.trim() === '' ? null : { kind: 'copy', text: facts.selection }
  }

  if (letter(ev, 'v')) {
    /*
     * Both kinds, and `facts.kind` no longer decides it here.
     *
     * It used to: a Claude pane returned `null` so `^V` reached the CLI's own paste, image
     * included. That left every Claude prompt *other than* the main one — the numbered-choice
     * questions, the permission prompts — with a Ctrl+V that did nothing, because nothing down
     * there was listening for the byte. See the header.
     *
     * The kind still decides the *fallback*, but that decision is `pasteIntoTerminal`'s: it
     * needs to know whether the clipboard holds text, and that is an `await`. This function is
     * called from xterm's custom key handler, which is synchronous and whose return value is
     * the only thing that stops `^V` reaching the pty — so the decision to intercept has to be
     * made before the answer exists. Intercept always, re-emit `^V` when there was no text: the
     * image path costs one round trip of latency and nothing else.
     */
    return { kind: 'paste' }
  }

  return null
}

/**
 * Whether this keystroke should open the pane's find bar instead of reaching the child.
 *
 * Separate from [`terminalClipboardAction`] rather than a third `ClipboardAction` variant: that
 * type is about the system clipboard and its two branches both end in `terminal/clipboard.ts`,
 * whereas this one ends in a piece of pane chrome. Folding them would make one function's name
 * a lie and force every caller of either to know about both.
 *
 * Same shape as everything else here — plain Ctrl only, `key` rather than `code`, keydown only —
 * and for the same three reasons, which the module header states once.
 *
 * `true` means the caller must return `false` to xterm. `^F` is a byte a child wants (readline's
 * `forward-char`), so nothing else stops it.
 */
export function terminalOpensFind(ev: TerminalKeyEvent): boolean {
  if (ev.type !== 'keydown') return false
  if (!plainCtrl(ev)) return false
  return letter(ev, 'f')
}
