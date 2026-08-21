/**
 * The editor's own key bindings, as data.
 *
 * Two arrays and no DOM. That is the whole reason this file exists rather than these bindings
 * living beside the code that installs them: `find.ts` imports `./EditorSurface.module.css` and
 * `EditorSurface.tsx` imports React, so neither can be `require`d by a check script — and a
 * keymap that can only be asserted about with a regex over its source is a keymap whose chords
 * nobody has ever actually resolved. `check-editor.mjs` loads this module, folds both arrays
 * into the same `claimedOn(platform)` expansion it already runs over the upstream packages, and
 * compares the resolved command **by identity**. It is the same split, and for the same reason,
 * that `find.ts`'s own header draws between itself and `findMatches.ts`.
 *
 * `lineEditKeymap` is installed by three surfaces — `EditorSurface`, `DiffPane` and
 * `MergePane` — which is the second reason: three copies of one binding drift, and the one that
 * drifts is the one nobody opened.
 */
import { copyLineDown, copyLineUp, moveLineDown, moveLineUp } from '@codemirror/commands'
import { gotoLine, searchKeymap, selectNextOccurrence } from '@codemirror/search'
import type { KeyBinding } from '@codemirror/view'

/**
 * The three line commands every editable surface gets: duplicate it, and move it up or down.
 *
 * # Ctrl+D — duplicate the line, or the selection. IDEA's chord for IDEA's command.
 *
 * `copyLineDown` is `@codemirror/commands`' own, and `defaultKeymap` also binds it to
 * `Shift-Alt-ArrowDown`. That second chord is **taken away below** and given to move-line, which
 * is what IDEA spells it — so Ctrl+D is not an addition beside it, it is where duplicate-line
 * lives now. `Mod-Alt-d` beside it is duplicate *above*, re-homed off the same chord.
 *
 * # Why this is a CodeMirror binding and not a `cide-core::commands` id
 *
 * A binding in `keymap.rs` is resolved by `keys/gate.ts`'s window **capture** listener before
 * any text surface is offered the event, and plain `ctrl+d` is a byte a pty wants: `^D` is EOF,
 * and `keymap.rs`'s rule is that xterm encodes a control byte for `ctrl && !shift && !alt &&
 * !meta`. An id here would take EOF from every terminal pane in every window — which is exactly
 * why `gitlog/LogView.tsx` refuses to register *its* Ctrl+D, and it says so at the binding.
 *
 * `when: "editorFocused"` would not save it either, and would break the other half of the
 * feature: `editorFocused` is a **pane-kind** flag (`keys/context.ts`), so a diff pane and a
 * merge pane never raise it, and those are two of the three surfaces this array is installed in.
 *
 * The cost, stated rather than discovered later: **no palette row and no menu item.** Giving it
 * an id means a `when`, a dispatch arm and a route from `keys/dispatch.ts` into one pane's live
 * `EditorView` — the seam `paneHosts.ts` exists to keep closed. Move-line pays it too, for the
 * same reason and with the same `^D`-shaped argument standing behind the whole array.
 *
 * `preventDefault` because Ctrl+D is *Bookmark this page* to a webview that has no bookmarks.
 *
 * # Ctrl+Shift+Up / Ctrl+Shift+Down — move the line, or the selected block
 *
 * The compensation `cide-core::keymap` has claimed since M12: `alt+up`/`alt+down` are
 * `navigate.prevMember`/`navigate.nextMember` in the app keymap, and the key gate is a window
 * **capture** listener, so `defaultKeymap`'s `Alt-ArrowUp`/`Alt-ArrowDown` never see the event.
 *
 * These lived in `EditorSurface`'s own keymap array until M17, which made them true of exactly
 * one of the three editable surfaces this file exists to keep in step. A diff pane's proposed
 * side and a merge pane's result pane are both places a user edits text and both had Ctrl+D and
 * no way to move a line — *duplicate* was the only line command in the pane, which is the report
 * this moved for. They belong beside Ctrl+D for the reason the header gives, and they cost
 * nothing extra to put there: **`moveLine` guards on `state.readOnly` and returns `false`**,
 * exactly as `copyLine` does, so the read-only `a` side and the two conflict sides refuse them
 * without a second array.
 *
 * ## Why the chord is spelled twice
 *
 * `Mod-Shift-Arrow` is free on Linux and Windows and is **not** free on macOS: `standardKeymap`
 * carries `{ mac: 'Cmd-ArrowUp', shift: selectDocStart }`, so ⌘⇧↑/⌘⇧↓ are select-to-top-of-file
 * and select-to-bottom there. This array is offered the stroke before `defaultKeymap` in all
 * three surfaces, so spelling it `Mod-` would take those two away silently.
 * `Mod-Alt-Shift-Arrow` is free in both layers — checked in `check-editor.mjs` by expanding the
 * composed keymap the way CodeMirror expands it, `shift:` sub-bindings and per-platform `mac:`
 * spellings included, rather than by reading the documentation.
 *
 * ## And IDEA's own spelling, ⌥⇧↑/⌥⇧↓ — which took two goes to get right
 *
 * `Shift-Alt-Arrow` is IDEA's chord for *Move Line Up/Down* on every platform, and in
 * `defaultKeymap` it is `copyLineUp`/`copyLineDown` — **duplicate the line above / below**. So
 * the chord a user reaches for to move a line duplicated it instead, which is the report this
 * array has now been filed under twice. The first fix bound `Mod-Shift-Arrow` and left this
 * chord alone; the comment here said IDEA's spelling "loses" because taking it would trade one
 * capability for another. That reasoning was sound **when duplicate-line had no other chord**
 * and stopped being sound the moment Ctrl+D landed:
 *
 * - `copyLineDown` — *duplicate below*, the one users mean — is **Ctrl+D**, above.
 * - `copyLineUp` — *duplicate above* — is re-homed to **Ctrl+Alt+D**, free in both layers and
 *   unbound in `cide_core::keymap` (both computed, not asserted from this paragraph).
 *
 * Nothing is deleted, both chords now move a line, and `defaultKeymap`'s two entries are
 * shadowed rather than removed: this array is spread ahead of it in all three surfaces, and
 * CodeMirror runs same-chord commands in array order until one returns `true`.
 *
 * **Why bind two chords for one command at all.** `Mod-Shift-Arrow` is the compensation the app
 * keymap owes; `Shift-Alt-Arrow` is the chord muscle memory actually reaches for, and it is the
 * one that was doing the wrong thing. Leaving it on copy-line means the failure the re-homing
 * was built to fix goes on happening for anyone who never reads a keymap — which is everyone.
 */
export const lineEditKeymap: readonly KeyBinding[] = [
  { key: 'Mod-d', run: copyLineDown, preventDefault: true },
  { key: 'Mod-Alt-d', run: copyLineUp, preventDefault: true },
  { key: 'Mod-Shift-ArrowUp', mac: 'Mod-Alt-Shift-ArrowUp', run: moveLineUp },
  { key: 'Mod-Shift-ArrowDown', mac: 'Mod-Alt-Shift-ArrowDown', run: moveLineDown },
  { key: 'Shift-Alt-ArrowUp', run: moveLineUp },
  { key: 'Shift-Alt-ArrowDown', run: moveLineDown },
]

/**
 * What `find.ts` installs: `searchKeymap`, picked over, with one chord re-homed.
 *
 * `searchKeymap` was included whole, and the comment here used to say so as a principle. It is
 * now picked over by exactly two entries, and the honest version of the rule is: *everything
 * except a second user interface for something cide already has* — plus, now, one chord that a
 * more-wanted command took, which is a different kind of edit and is kept separate below.
 *
 * # What stays, and why each one earns it
 *
 * `Mod-f` opens the bar, `F3`/`Shift-F3` walk the matches, `Escape` closes it and `Mod-Shift-l`
 * selects every match. `Mod-g` is find-next as well, and it stays even though `cide-core::keymap`
 * now binds `ctrl+g` to `navigate.line` and the window capture gate therefore eats it before
 * CodeMirror is offered it in an editor pane. That is not a dead entry: a user who takes Ctrl+G
 * back with one line of `keymap.json` gets find-next on it again, for free, because it was never
 * removed. Deleting it would turn a rebindable trade into a permanent loss.
 *
 * # The one that goes: `Mod-Alt-g` → `gotoLine`
 *
 * Go to line is cide's now — `navigate.line`, in the registry, in the palette, on Ctrl+G, and
 * rebindable. Leaving CodeMirror's binding in place would give one action two user interfaces:
 * ours, and a stock `showDialog` panel that docks at the *bottom* of the pane and paints itself
 * from `.cm-panels`, whose colours are literals in CodeMirror's base theme — the exact hazard
 * `find.ts`'s header cites as the reason the search panel was replaced in the first place. Two
 * dialogs for one verb, one of which ignores the theme, is worse than either alone.
 *
 * # The one that moves: `Mod-d` → `Alt-j`
 *
 * `Mod-d` was `selectNextOccurrence` — VS Code's add-the-next-occurrence-to-the-selection — and
 * `lineEditKeymap` above now wants that chord for duplicate-line, which is what IDEA spells
 * Ctrl+D and what was actually asked for. So the capability **moves** rather than disappearing:
 * `Alt-j` is IDEA's own chord for *Add Selection for Next Occurrence*, and it is free in both
 * layers — `cide_core::keymap` binds only `ctrl+alt+j` (`pane.navigate.down`), and nothing in
 * the composed CodeMirror keymap claims it (`defaultKeymap` takes `Alt-l`, `Alt-A` and the
 * Alt-arrows; `historyKeymap` takes `Alt-u`). Both halves are *computed* by `check-editor.mjs`
 * rather than trusted from this paragraph, because a paragraph in this position claiming a chord
 * was free is precisely what was wrong about move-line for four milestones.
 *
 * No `mac:` spelling, and that is deliberate rather than an omission. Option rewrites
 * `event.key` on macOS — ⌥J is `∆` — but `@codemirror/view`'s `runHandlers` falls back to the
 * `keyCode` base name for exactly this case, so `Alt-j` resolves there too. A second spelling
 * would be a second thing to keep true.
 *
 * # Both filters are by command identity, never by key string
 *
 * `binding.key === 'Mod-Alt-g'` would stop matching the day upstream re-spells the chord, and
 * would stop matching *silently* — the stock dialog would simply reappear. Comparing against the
 * imported `gotoLine` and `selectNextOccurrence` cannot drift.
 *
 * The re-homed entry goes **first**, before the spread, so that it wins on precedence if
 * upstream ever binds `Alt-j` to something of its own; the filter is what guarantees the
 * *removal*, and the ordering is what guarantees the *replacement*.
 */
export const searchBindings: readonly KeyBinding[] = [
  { key: 'Alt-j', run: selectNextOccurrence, preventDefault: true },
  ...searchKeymap.filter(
    (binding) => binding.run !== gotoLine && binding.run !== selectNextOccurrence,
  ),
]
