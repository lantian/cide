//! Layered keybinding resolution over the VS Code keymap format.
//!
//! Three layers, later wins: compiled-in [`defaults`], then [`platform_defaults`] (which
//! rewrites those defaults for macOS and is empty elsewhere), then the user's
//! `keymap.json`. Every surviving binding remembers the layer it came from so Settings →
//! Keymap can show what has been overridden.
//!
//! The one invariant that matters here is that *keys are compared normalised*. `Ctrl+Shift+P`,
//! `shift+ctrl+p` and `ctrl+shift+p` are the same keystroke, and a user override that spells
//! its modifiers differently from the default it means to replace must still replace it.
//! Comparing the raw strings would leave both bindings alive, which presents to the user as
//! "my keybinding does nothing" — a failure with no visible cause. So [`resolve`] normalises
//! every key it stores and [`conflicts`] normalises again before grouping.
//!
//! Shadowing is deliberately across layers only. Two bindings on one key *within* a layer
//! both survive, so that a user who binds the same keystroke twice in one file gets a
//! [`Conflict`] to look at rather than a silent winner.

use std::fmt;
use std::path::Path;

use cide_ipc::{Binding, KeymapEdit, KeymapLayer, ResolvedBinding};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, Result, persist};

/// One keystroke: a set of modifiers plus a key name.
///
/// A binding's `key` is a sequence of these — `ctrl+k ctrl+s` is two chords — because the
/// frontend needs the strokes separately to run its prefix state machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// Lowercased key name, e.g. `p`, `right`, `comma`, `` ` ``. Never empty in anything
    /// [`parse_chord`] returns; the derived `Default` and the public fields do not enforce
    /// it, so a hand-built `Chord` with an empty key renders as a bare modifier string.
    pub key: String,
}

impl fmt::Display for Chord {
    /// Writes the canonical spelling: modifiers in `ctrl alt shift meta` order, then the key.
    ///
    /// This ordering is the whole of normalisation for a single stroke, so it must not
    /// depend on how the binding was written.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (present, name) in [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (self.meta, "meta"),
        ] {
            if present {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Two or more commands competing for the same keystroke in the same context.
///
/// Reported rather than resolved: which of them should win is a judgement only the user can
/// make, and picking one silently would hide the mistake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Conflict {
    /// The normalised key the commands share.
    pub key: String,
    /// Distinct command ids, in resolution order — the last one is the one that will win.
    pub commands: Vec<String>,
    /// The `when` clause all of them carry, `None` when all of them are unconditional.
    pub when: Option<String>,
}

/// The compiled-in keymap: every command the design mock's palette shows a shortcut for.
///
/// Spelled for Linux and Windows. macOS gets its `⌘` spellings from [`platform_defaults`],
/// so nothing in this list is platform-conditional.
pub fn defaults() -> Vec<Binding> {
    [
        ("ctrl+alt+right", "pane.split.right"),
        ("ctrl+alt+down", "pane.split.down"),
        ("ctrl+shift+n", "claude.split.newSession"),
        // `ctrl+shift+up` was here for `pane.promoteToTab` and is deliberately gone: that
        // command is `unavailable`, and a key bound to it is swallowed by the gate and does
        // nothing — worse than an unbound key, which at least reaches whatever is underneath.
        // `commands::nothing_binds_a_key_to_an_unavailable_command` keeps it gone.
        ("ctrl+shift+d", "pane.detachToWindow"),
        ("ctrl+shift+`", "terminal.splitBelow"),
        /*
         * The four spawn chords, asked for by name: "CTRL+( - new claude panel, CTRL+) - new
         * claude row, CTRL+{ - new bash panel, CTRL+} - new bash row". A "panel" is a tile
         * beside the focused pane — the pane title bar's `⊞` — and a "row" is the header's
         * full-width `⊞ bash row` / `⊞ claude row`, so these are the keyboard spellings of the
         * four existing mouse gestures.
         *
         * # Spelling
         *
         * The *physical* keys, not the shifted faces. `strokeFromEvent` reads
         * `KeyboardEvent.code` first, so no keystroke ever produces the token `(` — on a US
         * layout Ctrl+( arrives as `ctrl+shift+9` — and a binding written with the literal
         * would be inert, the trap the `~` note in `chords.ts`'s `ALIASES` documents. That
         * table folds `(`, `)`, `{` and `}` onto `9`, `0`, `bracketleft` and `bracketright`
         * now, so the ask's own spellings still mean these chords in a `keymap.json`.
         *
         * # What they cost, checked in every layer rather than assumed
         *
         * * **In this table**: nothing bound any ctrl+shift+digit or ctrl+shift+bracket, in
         *   any scope.
         * * **In a terminal**: nothing. xterm encodes a control byte only for
         *   `ctrl && !shift && !alt && !meta` (`Keyboard.ts`) — the same fact that leaves
         *   ⌃⇧C inert — so no pty loses a byte. The unshifted keys are another story
         *   entirely: Ctrl+[ **is** ESC and Ctrl+] is telnet's escape, which is why the ⌘[
         *   note in `platform_layer` refuses them; Shift is what makes these four free.
         * * **In CodeMirror**: `Ctrl-Shift-[` / `Ctrl-Shift-]` are `foldKeymap`'s chords
         *   upstream, and `editor/folding.ts` deliberately does not install it — folding
         *   lives on the `ctrl+minus` family below. Nothing binds a `Mod-Shift-` digit.
         * * **On macOS**: the blanket rewrite makes ⇧⌘9 / ⇧⌘0 / ⇧⌘[ / ⇧⌘], none of which
         *   AppKit's default menu claims (`MACOS_MENU_CHORDS`). ⇧⌘[ and ⇧⌘] are every Cocoa
         *   tab bar's previous/next-tab reflex; cide draws no native tabs, so the chords
         *   land — a divergence recorded here like ⌘T's, not worked around.
         */
        ("ctrl+shift+9", "claude.split.right"),
        ("ctrl+shift+0", "claude.addRow"),
        ("ctrl+shift+bracketleft", "terminal.splitRight"),
        ("ctrl+shift+bracketright", "terminal.addRow"),
        ("ctrl+p", "picker.files"),
        ("ctrl+shift+p", "palette.commands"),
        // Find in files. The search panel was built, wired to its Rust engine, and reachable
        // from exactly one place: the ⌕ button in the activity rail. No command, no binding,
        // and no way to put the caret in its box without a mouse.
        //
        // `ctrl+shift+f` was free in every layer that could have claimed it — nothing else in
        // this table uses `f` at all, CodeMirror's `searchKeymap` binds `Mod-f` and `F3` but
        // not `Mod-Shift-f`, and xterm claims nothing. On macOS the rewrite below turns it into
        // ⇧⌘F, which is the same gesture every editor on that platform uses for the same thing,
        // so it needs no exception in `keeps_ctrl_on_macos`.
        ("ctrl+shift+f", "sidebar.search"),
        // *Select opened file*, and the same story as `ctrl+shift+f` above: the command existed,
        // was dispatched, read its answer and reported a failure — and could be reached only by
        // typing its name into the palette **while an editor pane had focus**. The button in the
        // Explorer header and this chord are the two gestures that make it a feature.
        //
        // # What it costs: nothing, in any of the three layers
        //
        // Nothing in this table uses `e` at all. CodeMirror's installed keymaps
        // (`closeBracketsKeymap`, `defaultKeymap`, `historyKeymap`, `indentWithTab`, and a
        // filtered `searchKeymap`) bind `Mod-Shift-` only for `l`, `u` and `z`. And xterm sends
        // **no bytes**: `Keyboard.ts` encodes a control character only for
        // `ctrlKey && !shiftKey && !altKey && !metaKey`, which is exactly what leaves
        // Ctrl+Shift+C and Ctrl+Shift+V inert in a terminal today.
        //
        // # Unconditional, deliberately
        //
        // `Command::when` gates the *palette*; a `Binding::when` here would gate the keyboard,
        // and an `editorFocused` clause would make the chord dead in a terminal and in the file
        // tree — which is where somebody asking "where is the file I am editing?" most often
        // has their hands. Nothing is taken from anyone, so there is nothing to scope.
        //
        // # The IDEA divergence, named rather than discovered
        //
        // ⌃⇧E is *Recent Locations* in IDEA; *Select Opened File* has no default chord there at
        // all (it is a gear-menu action). cide has no Recent Locations, so nothing is lost — but
        // a user coming from IDEA will press this expecting one, and `README.md` says so. On
        // macOS `platform_layer` rewrites it to ⇧⌘E, which is free there too and needs no
        // exception in `keeps_ctrl_on_macos`.
        ("ctrl+shift+e", "file.reveal"),
        // A new scratch file, on IDEA's own chord for it.
        //
        // # What it costs a terminal, precisely
        //
        // xterm claims nothing here; it *encodes*. Shift+Alt+S becomes `ESC` `S`
        // (`@xterm/xterm`'s `Keyboard.ts`), and the gate is a window **capture** listener, so
        // those two bytes are what this binding takes from every pane in every window. `ESC S`
        // is unbound in stock readline. In vim it is *leave insert mode, then* `S`, which
        // substitutes the current line — so for a vim user this binding arguably prevents a
        // keystroke rather than costing one. Rebindable either way:
        // `{"key":"alt+shift+s","command":"-scratch.new"}` in `keymap.json`.
        //
        // Free everywhere else. This table has no `alt+shift+*` binding at all, and a sweep of
        // `@codemirror/{commands,search,autocomplete,language}` finds no `Alt-s` in any
        // spelling. `platform_layer` only rewrites keys containing `ctrl`, so this passes
        // through untouched on macOS — where ⌥⇧S is also free.
        ("alt+shift+s", "scratch.new"),
        ("ctrl+w", "tab.close"),
        ("ctrl+s", "file.save"),
        // Reopen the last closed tab, which is what this chord means in every browser and in
        // every IDE that has the gesture. It was `theme.toggle`, and that command is now
        // **palette-only**: switching the theme is a thing a user does twice a year, and it was
        // holding the one chord whose muscle memory is universal.
        //
        // Nothing in this workspace requires a registered command to carry a binding —
        // `check-commands.mjs` checks the other direction only, that every bound key names a
        // registered and available command — so `theme.toggle` losing its default costs it
        // nothing but the chord. `project.switcher.prev` is the existing precedent and
        // `the_default_layer_binds_every_command_the_palette_shows` names both.
        ("ctrl+shift+t", "tab.reopenClosed"),
        // Pull, asked for by name: "need hotkey CTRL+T for git pull".
        //
        // # What it costs, precisely
        //
        // Ctrl+T is not free. It is readline's `transpose-chars`, it is **fzf**'s default
        // file-widget trigger, and it is vim's tag-jump — and a binding here kills all three
        // in **every pane, in every window, unconditionally**, including inside the Claude
        // pane. The gate's window listener resolves the chord before xterm is offered the
        // event at all (`ui/src/keys/gate.ts`), so nothing reaches the pty: this is not
        // scoped to bash line editing and there is no arrangement in which it is.
        //
        // That trade is already made three times over in the table above, and unconditionally
        // each time: `ctrl+p` takes readline's previous-history, `ctrl+w` takes
        // unix-word-rubout (the most severe of the four), `ctrl+s` takes terminal XOFF flow
        // control. There is not one `.when()` in this function. A `when("!terminalFocused")`
        // was considered and is the worst of both here — the flag exists and works, but it
        // would make the hotkey dead exactly where a terminal-centric app puts the user's
        // hands, while still costing the reader a conditional to understand.
        //
        // It stays rebindable, which is the whole answer to the cost: a line of
        // `~/.config/cide/keymap.json` — `{"key":"ctrl+t","command":"-git.pull"}` — puts
        // transpose-chars back. `cide_app::cmd::app::user_keymap` reads that file and
        // `resolve` applies the removal.
        //
        // On macOS `platform_layer` below unbinds `ctrl+t` and binds `⌘T` instead, which is
        // right for Cocoa's own transpose and wrong for every user's "new tab" reflex. No
        // code covers that; it is a known cost of the blanket rewrite, not of this line.
        ("ctrl+t", "git.pull"),
        ("ctrl+alt+h", "pane.navigate.left"),
        ("ctrl+alt+l", "pane.navigate.right"),
        ("ctrl+alt+k", "pane.navigate.up"),
        ("ctrl+alt+j", "pane.navigate.down"),
        /*
         * Alt+Left / Alt+Right — the same two horizontal moves, on the arrows. Asked for by
         * name: "ALT+UP/DOWN/LEFT/RIGHT should allow to move focus between panels". The vim
         * spellings above stay — a moved chord breaks the muscle memory this table shipped
         * first, and two keys for one command is a state `conflicts` has no opinion about
         * (it groups commands per key, never keys per command).
         *
         * The vertical pair is **not** here: `alt+up`/`alt+down` are the member walk inside
         * a buffer, so those two live in the `when`-carrying block below as the complement
         * `!editorFocused`, beside the rows they complement. These two are unconditional on
         * purpose — an editor pane needs *some* alt+arrow that still leaves it, or the
         * keyboard is stranded inside a buffer and the feature answers only half its ask.
         *
         * # What it costs, checked in every layer rather than assumed
         *
         * * **In `defaults()`**: nothing bound any bare Alt+Arrow. Free.
         * * **In a terminal**: xterm encodes Alt+Left as `ESC [ 1 ; 3 D` (`… C` for Right) —
         *   word-motion in zsh's default line editor and in many a distro's inputrc — and
         *   the gate is a window **capture** listener, so every pane in every window loses
         *   those bytes, unconditionally. The `mouseback` note below refused exactly this
         *   trade for `navigate.back`, a command nobody had asked to put there; here the
         *   chord was asked for by name, which is the `ctrl+t` trade again, and the same
         *   escape hatch pays for it: `{"key":"alt+left","command":"-pane.navigate.left"}`.
         * * **In CodeMirror**: `defaultKeymap` binds `Alt-ArrowLeft`/`Alt-ArrowRight` to
         *   `cursorSyntaxLeft`/`Right`, and the gate resolves first, so syntax-step motion
         *   goes. Taken deliberately: it is the least-known motion in the default set, and
         *   losing it is what buys the way *out* of a buffer. `Shift-Alt-Arrow` is a
         *   different stroke, so select-syntax and move-line survive untouched.
         * * **On macOS**: no `ctrl`, so `platform_layer` passes both through — where ⌥←/⌥→
         *   is word-motion in every Cocoa text view and in CodeMirror's mac spellings. A
         *   real divergence, recorded rather than worked around, like F4's Fn tax.
         */
        ("alt+left", "pane.navigate.left"),
        ("alt+right", "pane.navigate.right"),
        ("ctrl+comma", "settings.open"),
        // M12. `ctrl+f12` is IDEA's own File Structure chord; `ctrl+alt+shift+n` is free in every
        // layer (`ctrl+shift+n` is `claude.split.newSession`, and the two normalise to different
        // keys).
        //
        // This comment used to claim that no f-key was bound anywhere else in this workspace.
        // `f4` is, in the `when`-carrying block below, and the claim is kept here as a *pointer*
        // rather than deleted: `f3` and `shift+f3` are still the find bar's and still forbidden
        // outright by `nothing_binds_the_find_bars_f_keys`.
        ("ctrl+f12", "structure.file"),
        ("ctrl+alt+shift+n", "picker.symbols"),
        // The mouse's thumb buttons, as keys. (M12)
        //
        // # Why they are in the keymap at all
        //
        // They are not keys and they are bound here anyway, because the alternative is a second
        // place where input is turned into a command. `ui/src/keys/gate.ts` has *one* resolver
        // with two entry points that `check-key-gate.mjs` holds to the same answer for every
        // chord; a mouse handler that named a command directly would be a third path with its
        // own copy of the prefix machine, the `when` evaluation and the dispatch. Instead the
        // Rust side names a *button* — exactly as xterm's handler names a keystroke — and the
        // gate resolves it through the same table.
        //
        // Both parsers already accept any key token (`parse_stroke` here validates modifiers
        // only, and `ui/src/keys/chords.ts::parseStroke` does the same), so this needs no parser
        // change and the user gets the whole keymap vocabulary for free: modifiers compose
        // (`ctrl+mouseback`), a rebind is one line of `keymap.json`
        // (`{"key":"mouseback","command":"navigate.definition"}`), and so is an unbind
        // (`{"key":"mouseback","command":"-navigate.back"}`).
        //
        // # Why they carry no `when`
        //
        // Unlike every other M12 binding these are *not* `editorFocused`. A thumb button is
        // pressed wherever the pointer is, which in this app is most often over a terminal, and
        // there is nothing to take away: buttons 8 and 9 reach no pty and no shell reads them.
        // The one precondition that matters — somewhere to go back to — cannot be a context flag
        // and is re-checked in `keys/dispatch.ts`.
        //
        // # Why no keyboard chord is bound to these
        //
        // IDEA's Ctrl+Alt+Left/Right is not free here: `ctrl+alt+right` is already
        // `pane.split.right` above, and on KDE both are commonly the compositor's
        // virtual-desktop shortcuts and never reach the app. `alt+left`/`alt+right` are
        // word-motion in readline — a cost this table refused to pay for a navigation nobody
        // had asked to put there, and later *did* pay for `pane.navigate.left`/`right` above,
        // where the chord was asked for by name; either way the pair is spent.
        // `ctrl+alt+shift+left`/`right` are free in every layer and are
        // what a user should add if they want them — `README.md` prints the two lines. Shipping
        // an unbound pair rather than guessing is the same trade `file.saveAll` already makes.
        ("mouseback", "navigate.back"),
        ("mouseforward", "navigate.forward"),
    ]
    .into_iter()
    .map(|(key, command)| Binding::new(key, command))
    // Bindings that carry a `when`, and the only ones in this table that do.
    //
    // Three different reasons live in this block and mixing them up would be easy, so they are
    // named once here. `editorFocused` (the M12 group) is about *taking a key away from a
    // terminal*: the gate is a window capture listener, so an unscoped chord is swallowed in
    // every pane in every window, and `ctrl+b` is tmux's prefix and `ctrl+g` is readline's
    // abort. `shellWindow` (the M14 group) is about *a window that has nowhere to put the
    // gesture*: a detached pane or tab window has no rail, no sidebar and no project strip, and
    // `App.tsx` installs the gate before it branches on the role — so without the clause those
    // chords are swallowed there in exchange for a diagnostic log line. And `!editorFocused`
    // (the pane-gesture trio) is the first reason's mirror image: *giving a key to everything
    // except the buffer*, because inside one the same chord is already a feature somebody asked
    // for — its own comment below walks through it.
    //
    // # Why alt+up / alt+down need a clause when nothing above does
    //
    // `alt+up` / `alt+down` are IDEA's *Previous/Next Method*. Unconditionally bound they would
    // also fire in a terminal pane, where the gesture would move a caret the user cannot see —
    // and the key gate is a window **capture** listener, so a globally-bound chord never reaches
    // the pane that should have had it.
    //
    // # What this costs, named rather than hidden
    //
    // `@codemirror/commands` binds `Alt-ArrowUp`/`Alt-ArrowDown` to `moveLineUp`/`moveLineDown`,
    // so inside a buffer those stop moving lines. `EditorSurface` re-homes them, so a capability
    // moves rather than disappearing.
    //
    // This paragraph used to say the re-homing was to `Mod-Shift-Arrow`, "IDEA's own chord …
    // free in both layers". Both halves were wrong and it took until M16 to notice: `grep` found
    // no `moveLineUp` anywhere in `ui/src`, so the compensation had never been built and
    // move-line was simply gone; and `Mod-Shift-Arrow` is *not* free on macOS, where
    // `standardKeymap`'s `{ mac: "Cmd-ArrowUp", shift: selectDocStart }` makes ⌘⇧↑/⌘⇧↓
    // select-to-top and select-to-bottom. The binding that exists now is `Mod-Shift-Arrow` on
    // Linux and Windows and `Mod-Alt-Shift-Arrow` on macOS, which is free in both — checked by
    // expanding the composed CodeMirror keymap in `check-editor.mjs` rather than by reading its
    // documentation, because reading it is what produced the sentence this paragraph replaces.
    //
    // The alternative was `ctrl+alt+up`/`ctrl+alt+down`, and it costs strictly more: `ctrl+alt+down`
    // is already `pane.split.down` above, *and* CodeMirror binds `Mod-Alt-Arrow` to
    // `addCursorAbove`/`addCursorBelow`. Two breakages against this one.
    .chain(
        [
            ("alt+down", "navigate.nextMember", "editorFocused"),
            ("alt+up", "navigate.prevMember", "editorFocused"),
            /*
             * Alt+Up / Alt+Down as pane moves and Alt+Enter as maximize — the complement of
             * the member walk directly above. Asked for by name: "hotkey ALT+ENTER that will
             * do the current panel (claude or bash) to fullscreen or back to normal", and
             * "ALT+UP/DOWN/LEFT/RIGHT should allow to move focus between panels".
             *
             * # Why `!editorFocused`, the first negated clause in this table
             *
             * All three chords already mean something inside a buffer, and each meaning was
             * itself asked for by name in its own round: `alt+up`/`alt+down` are the member
             * walk two rows up, and `Alt-Enter` is *send lines to Claude*
             * (`EditorSurface`'s own keymap; the gate is a window **capture** listener, so an
             * unscoped row here would not shadow that feature, it would kill it in every
             * buffer in every window). `!editorFocused` is the two requests coexisting: a
             * buffer keeps its three chords, and every other pane — the claude and shell
             * panes the request names, and the diff and merge surfaces too, which are
             * `kind == "editor"` and so stay out — gets the moves and the toggle. The
             * horizontal arrows in the main table are the deliberate exception, and their
             * comment names the reason: an editor pane needs some alt+arrow that still
             * leaves it.
             *
             * `conflicts` reports nothing for `alt+up`/`alt+down` carrying two commands
             * each: `editorFocused` and `!editorFocused` are two scoped groups, and the fold
             * that makes an *unscoped* row contest every scoped one does not apply. That
             * silence is honest only because the clauses are genuinely disjoint — one flag,
             * negated — not two spellings that overlap in some state nobody tried.
             *
             * # What it costs a terminal, precisely — because there the chords now *fire*
             *
             * Alt+Up/Down are `ESC [ 1 ; 3 A/B`, which stock readline and zsh leave unbound;
             * near-free. Alt+Enter is `ESC` `CR`, which the Claude CLI reads as *insert
             * newline* in its composer — a real loss in exactly the pane this request is
             * about, taken with eyes open: `\` + Enter still inserts one (and Shift+Enter
             * after `/terminal-setup`), and one line of `keymap.json` gives the chord back —
             * `{"key":"alt+enter","command":"-pane.maximize","when":"!editorFocused"}`, the
             * clause repeated, or the removal matches nothing.
             *
             * On macOS `platform_layer` rewrites only chords containing `ctrl`, so all three
             * pass through untouched, and none of them is in `MACOS_MENU_CHORDS`.
             *
             * `pane.maximize` is a *toggle* — `keys/dispatch.ts` un-maximizes when the
             * focused pane already is the maximized one — so one chord is the whole gesture.
             * And the arrows compose with it rather than stranding anyone: focusing any
             * other pane un-maximizes (`layout::focus_pane`), so Alt+Arrow out of a
             * full-screen pane lands on a visible tile, never behind one.
             */
            /*
             * Move the pane, rather than the focus. (M31)
             *
             * The vim block above one modifier up: `ctrl+alt+<hjkl>` focuses a pane,
             * `ctrl+alt+shift+<hjkl>` moves it. What it costs in each layer:
             *
             * * **In `defaults()`**: nothing. The only Alt-bearing letters bound anywhere in
             *   this table are `alt+shift+s`, `alt+shift+d`, `alt+shift+f`, `ctrl+alt+shift+n`
             *   and `ctrl+alt+shift+s`, plus the `ctrl+alt+<hjkl>` four directly above. No
             *   `h`, `j`, `k` or `l` with Shift. Free.
             * * **In the terminal**: `ESC` + a shifted letter, which readline and tmux leave
             *   alone — and the clause below means a terminal is the only place these fire, so
             *   this is the whole of the cost.
             * * **In CodeMirror**: unreachable. `terminalFocused` is false in a buffer.
             * * **On macOS**: `platform_layer` rewrites `ctrl`-bearing chords it names, and it
             *   names none of these; none is in `MACOS_MENU_CHORDS` either.
             *
             * **Not `alt+shift+<arrow>`**, which reads better and is spent: the note on
             * `alt+left` above banks that stroke deliberately — *"`Shift-Alt-Arrow` is a
             * different stroke, so select-syntax and move-line survive untouched"* — and
             * taking it here would make that sentence false. **Not `ctrl+alt+shift+left`/
             * `right` either**: `README.md` prints those two lines twice as the binding a user
             * should add for `navigate.back`/`forward`, so a default there would collide with
             * advice already in the manual.
             *
             * `terminalFocused` rather than the `!editorFocused` its neighbours carry, and it
             * is the stronger clause: it excludes diff and image panes too, so nothing is taken
             * from any surface that is not a terminal — and it is the same flag that decides
             * whether the pane draws a grab handle, so the chord and the button agree.
             */
            ("ctrl+alt+shift+h", "pane.move.left", "terminalFocused"),
            ("ctrl+alt+shift+l", "pane.move.right", "terminalFocused"),
            ("ctrl+alt+shift+k", "pane.move.up", "terminalFocused"),
            ("ctrl+alt+shift+j", "pane.move.down", "terminalFocused"),
            ("alt+up", "pane.navigate.up", "!editorFocused"),
            ("alt+down", "pane.navigate.down", "!editorFocused"),
            ("alt+enter", "pane.maximize", "!editorFocused"),
            // IDEA's own Go to Declaration chord, and free in both layers: no binding in
            // `defaults()` uses `b`, and CodeMirror's `Ctrl-b` lives only in `emacsStyleKeymap`,
            // which `standardKeymap` re-exposes under a **mac-only** `mac:` property — so on
            // Linux and Windows nothing is being taken from anyone.
            //
            // Scoped to `editorFocused`, and the `when` is doing real work here: unscoped, Ctrl+B
            // would be swallowed by the window capture gate in every terminal pane in every
            // window, which is `tmux`'s prefix key. A user running tmux inside cide would lose it
            // everywhere, to a command that needs a caret to mean anything.
            //
            // On macOS `platform_layer` rewrites this to ⌘B, which is IDEA's mac binding for the
            // same action — correct by luck rather than by exception, but correct.
            ("ctrl+b", "navigate.definition", "editorFocused"),
            /*
             * Find usages — IDEA's own chord, unchanged. (M14)
             *
             * # What it costs, checked rather than assumed
             *
             * * **In `defaults()`**: nothing else uses an F-key but `f4` (the panel toggle), and
             *   nothing else uses Alt with one. Free.
             * * **In CodeMirror**: `defaultKeymap`, `historyKeymap`, `searchKeymap` and
             *   `closeBracketsKeymap` bind no F-key above F3, and none of them binds Alt with an
             *   F-key at all. Free.
             * * **In xterm**: F7 encodes as `ESC [ 18 ~` and Alt+F7 as `ESC ESC [ 18 ~`, so this
             *   *is* a byte a terminal application could want — `mc` puts its menu bar on the F
             *   keys. Which is exactly why the clause below is not optional.
             *
             * `editorFocused`, for the reason `ctrl+b` above states at length: the gate is a
             * window **capture** listener, so an unscoped chord is swallowed in every terminal
             * pane in every window, for a command that needs a caret to mean anything. With the
             * clause, a terminal keeps Alt+F7 and the buffer gets Find usages.
             *
             * macOS: `platform_layer` rewrites Ctrl to Meta and leaves Alt alone, so this stays
             * ⌥F7 — which is IDEA's mac binding for the same action, correctly and by luck.
             */
            ("alt+f7", "navigate.usages", "editorFocused"),
            /*
             * Next / previous change, in a diff or the conflict resolver. (M25)
             *
             * IDEA's own chords for Next/Previous Difference, and the cost checked in each layer
             * the way this table asks rather than assumed.
             *
             * * **In `defaults()`**: the only F-keys here are `f4` (`sidebar.toggle`) and
             *   `ctrl+f12` (`structure.file`), and `alt+f7` directly above. Bare `f7` and
             *   `shift+f7` are free, and neither displaces the other — a modifier is part of
             *   the stroke, so ⌥F7 goes on meaning Find usages in an editor.
             * * **In CodeMirror**: nothing installed here binds an F-key above F3, and the
             *   resolver's three panes are CodeMirror. Free.
             * * **In a terminal**: F7 is `ESC [ 18 ~`, a key `mc` puts a menu on. **This is why
             *   the clause is not optional**, and it is the same sentence ⌥F7's note above
             *   makes. The gate is a window *capture* listener, so `diffFocused` on its own
             *   would swallow F7 in a shell pane split beside a diff tab that happens to be in
             *   front. A diff pane and a merge pane are both `kind == "editor"`, so
             *   `terminalFocused` is false in exactly the states this needs and true in exactly
             *   the states a terminal needs it back — `!terminalFocused` therefore costs the
             *   feature nothing at all.
             * * **On macOS**: `platform_layer` only rewrites chords containing Ctrl, so bare
             *   F-keys pass through untouched, and F7 is not in `MACOS_MENU_CHORDS`. It is
             *   previous-track on the system keyboard, so it needs Fn — the same tax F4 already
             *   pays, and recorded in README's Platforms rather than worked around.
             *
             * The first compound clause in this table. To give F7 back:
             * `{"key":"f7","command":"-navigate.nextChange","when":"diffFocused && !terminalFocused"}`
             * — with the clause, because a removal matches on all three and one that forgets it
             * matches nothing.
             */
            (
                "f7",
                "navigate.nextChange",
                "diffFocused && !terminalFocused",
            ),
            (
                "shift+f7",
                "navigate.prevChange",
                "diffFocused && !terminalFocused",
            ),
            /*
             * Go to implementation — IDEA's own chord, unchanged. (M18)
             *
             * # What it costs, checked in every layer rather than assumed
             *
             * * **In `defaults()`**: the Ctrl+Alt entries are the three splits
             *   (`ctrl+alt+right`/`down`), the four pane moves (`ctrl+alt+h/j/k/l`) and
             *   `ctrl+alt+shift+n`. Nothing uses `ctrl+alt+b`. Free.
             * * **In CodeMirror**: `defaultKeymap` binds `Mod-b` nowhere on Linux — `Ctrl-b` lives
             *   only in `emacsStyleKeymap`, which `standardKeymap` re-exposes under a **mac-only**
             *   `mac:` property — and binds no `Mod-Alt` letter at all. Free.
             * * **On KDE, the development platform**: `ctrl+alt+l` is Lock Screen and is avoided
             *   elsewhere in this table for that reason; `ctrl+alt+b` is not a stock KDE global.
             * * **In xterm**: Ctrl+Alt+B is `ESC ^B`, which is readline's `backward-word` on a
             *   terminal that maps Alt to Escape-prefix. That is a real byte a shell user wants,
             *   which is precisely why the clause below is not optional.
             *
             * `editorFocused`, for the reason `ctrl+b` above states at length: the gate is a
             * window **capture** listener, so an unscoped chord is swallowed in every terminal
             * pane in every window, for a command that needs a caret to mean anything.
             *
             * `ctrl+b` keeps meaning "definition" and is untouched. Two questions, two chords —
             * see the id's own comment in `cide_core::commands` for why folding them into one
             * would regress Rust's most-used gesture to fix a Go complaint.
             *
             * macOS: `platform_layer` rewrites Ctrl to Meta and leaves Alt alone, so this becomes
             * ⌘⌥B — which is IDEA's mac binding for the same action, correctly and by luck.
             */
            ("ctrl+alt+b", "navigate.implementation", "editorFocused"),
            // Go to line, on IDEA's and VS Code's chord for it.
            //
            // # What it costs, named rather than discovered later
            //
            // Ctrl+G is not free *inside CodeMirror*: `@codemirror/search`'s `searchKeymap` binds
            // `Mod-g` to find-next. It is free everywhere else — nothing in this table uses `g`,
            // and xterm claims nothing — so the collision is entirely with the editor, and the
            // gate resolves before CodeMirror is offered the event, so find-next on this chord
            // goes.
            //
            // What survives is the point: `F3` and `Shift-F3` are find-next and find-previous in
            // `searchKeymap` too, both unbound here (`nothing_binds_the_find_bars_f_keys` keeps
            // them that way), and `ctrl+shift+g` normalises to a different stroke so
            // find-previous keeps that one as well. So the asymmetry this introduces, stated
            // plainly, is: **find-previous still answers Ctrl+Shift+G, find-next only F3.** That
            // trade is worth making because Ctrl+G is Go to line in both editors this keymap
            // follows, and because the same change made F3 work in the find field, where it had
            // never worked at all.
            //
            // Scoped to `editorFocused` for the reason spelled out for `ctrl+b` above, and it is
            // sharper here: `^G` is readline's `abort` and emacs' universal cancel, so an
            // unscoped binding would take the escape key of every shell running in every terminal
            // pane in every window, for a command that needs a caret to mean anything.
            //
            // On macOS `platform_layer` rewrites this to ⌘G mechanically. That is a *divergence*
            // rather than a lucky landing, and it is worth knowing which: IDEA-on-mac puts Go to
            // line on ⌘L and find-next on ⌘G. Left as the blanket rewrite produces it — a
            // platform exception here would need a matching one in `keeps_ctrl_on_macos` and
            // would be the only entry in that list not justified by the OS eating the chord.
            ("ctrl+g", "navigate.line", "editorFocused"),
            /*
             * Reformat code — Shift+Alt+F. (M26)
             *
             * VS Code's own chord for Format Document on Windows and macOS, and the one most
             * people arriving from another editor reach for. IDEA's Ctrl+Alt+L is not available:
             * it is KDE's Lock Screen on a stock install and never reaches the app, which this
             * table already recorded once at `alt+l`.
             *
             * # Why it is not `ctrl+alt+f`, which is what shipped first
             *
             * Because the first person to press that got nothing, silently. Their
             * `~/.config/kxkbrc` carries `altwin:swap_lalt_lwin` — an Apple keyboard on Linux —
             * so the key **labelled** Alt emits `meta` and the stroke arriving at the gate was
             * `ctrl+meta+f`, bound to nothing.
             *
             * **None of that is diagnosable from inside the app**, and that is the part worth
             * remembering rather than the chord: the gate correctly matches no binding, passes
             * the event through, and says nothing — which is indistinguishable from the feature
             * being broken, and was reported as exactly that. Separating "not wired" from "not
             * typable" took `cide-headless keymap`, a harness driving the real gate against the
             * real table, an empty `cide::ui` log, and finally reading `kxkbrc`.
             *
             * # What it costs, checked in every layer rather than assumed
             *
             * * **In `defaults()`**: the Alt+Shift entries are `alt+shift+d`, `alt+shift+s`,
             *   `ctrl+alt+shift+n` and `ctrl+alt+shift+s`. No `f`. Free.
             * * **In CodeMirror**: `editorKeys.ts` claims `Shift-Alt-ArrowUp`/`Down` for move-line
             *   and `defaultKeymap` claims the `Alt-Shift` arrow family; neither claims a
             *   `Shift-Alt` *letter*. `searchKeymap` claims `Mod-Shift-l` and nothing with Alt
             *   and Shift together. Free.
             * * **On KDE, the development platform**: `Alt+Shift+F` appears in
             *   `kglobalshortcutsrc` as `1-capture-full-page=none,Alt+Shift+F,Capture > Full
             *   Page`. The format is `active,default,description` and the **active field is
             *   `none`**, so it is a default that is not bound and the chord is free. It is a
             *   *latent* grab rather than a present one: enabling that capture would take the
             *   key globally and cide would never see it — the same class of failure this
             *   binding exists because of. `alt+shift+s` (`scratch.new`) has sat beside the same
             *   entry for Capture > Selected Area since M15 on exactly this reasoning.
             * * **In xterm**: Shift+Alt+F encodes as `ESC` `F`, the same shape the module header
             *   describes for Shift+Alt+S. Moot under the clause below, but a terminal is never
             *   offered the stroke rather than merely tolerating it.
             *
             * `editorFocused`, for the reason `ctrl+b` above states at length: the gate is a
             * window **capture** listener, so an unscoped chord is swallowed in every terminal
             * pane in every window, for a command that needs a buffer to mean anything.
             *
             * macOS: the chord carries no Ctrl, so `platform_layer` rewrites nothing and it
             * stays ⇧⌥F — which is exactly what VS Code binds Format Document to there.
             */
            ("alt+shift+f", "editor.format", "editorFocused"),
            /*
             * ⌥L — widen Ctrl+P to *External Libraries*, while Ctrl+P has the keyboard. (M16)
             *
             * # What it costs, checked in every layer rather than assumed
             *
             * * **In `defaults()`**: no binding uses Alt with a letter except `alt+shift+s`, and
             *   the other Alt entries are the arrows (the member walk and the pane moves),
             *   `alt+enter` and `alt+f7`. Free.
             * * **Against typing**: strokes come from `KeyboardEvent.code`, so this is
             *   layout-independent, and the gate resolves before the focused input is offered
             *   the event — the picker's field never sees a character.
             * * **Against the overlay's own keys**: `listAction` claims the arrows, the Page
             *   keys, Home/End, Enter and Escape, and nothing else. Free.
             * * **In terminals and in CodeMirror**: nothing at all, because of the clause. ⌥L is
             *   `ESC l` to a shell — readline's `downcase-word` — and taking that from every
             *   terminal pane in every window for a command that only means something while a
             *   modal is up would be the `ctrl+b` mistake with a smaller excuse.
             *
             * `ctrl+alt+l` was the obvious alternative and is rejected: it is KDE's Lock Screen
             * on a stock install, so on the development platform it never reaches the app at all.
             *
             * The clause names `filePickerOpen` rather than `overlayOpen`, which is true for any
             * of nine overlays. Scoped to the one that has a library scope, ⌥L in the palette,
             * the symbol picker or the branch popup stays unbound and passes through.
             *
             * `platform_layer` leaves Alt alone, so this stays ⌥L on macOS, where ⌥ is also
             * where an IDEA user's hand already is.
             */
            ("alt+l", "picker.libraries", "filePickerOpen"),
            /*
             * M14. Ctrl+Tab is the **tab** switcher, and this is a move rather than an addition:
             * `project.switcher.next` / `.prev` were bound here and are not any more.
             *
             * The user asked for both — "ctrl+tab over opened files", "ctrl+~ for projects" —
             * and the second half is what makes the first possible. Ctrl+Tab means "the thing
             * inside this window" in every browser and every IDE, and projects are the outer
             * ring; putting the inner ring on the chord everyone's fingers already know is the
             * whole point of the swap.
             *
             * **A user's `keymap.json` needs no migration and gets what they asked for**, by
             * two different mechanisms depending on what they wrote. `apply_layer` overrides on
             * (normalised key, normalised `when`), and this default now carries a clause, so a
             * user line naming `ctrl+tab` with no clause does not replace it — the two coexist,
             * which is deliberate and correct here: within a layer stack the *last applicable*
             * binding wins, the user's layer is last, and their unconditional line therefore
             * beats this one in a shell window and is the only one that applies anywhere else.
             * They keep the project switcher on Ctrl+Tab, the tab switcher is unreachable by key
             * for them, and `conflicts` reports nothing, because the two clauses differ.
             *
             * The one case that changes under such a user is an *unbind* of the old default. A
             * removal matches on (key, command, `when`), so a line unbinding
             * `project.switcher.next` from `ctrl+tab` now matches nothing: it is reported as
             * `RemovalMatchedNothing` in Settings → Keymap, and Ctrl+Tab starts running the tab
             * switcher they never asked for. That is the release note.
             *
             * `shellWindow`, unlike the unconditional binding this replaces. The strip is the
             * shell window's; a detached-pane window has no tabs and a detached-tab window has
             * one, and there activating a tab would move the *shell* window's active tab. With
             * the clause the chord is not merely inert in those windows, it is left alone — and
             * Tab in a torn-out terminal goes back to being Tab.
             */
            ("ctrl+tab", "tab.switcher.next", "shellWindow"),
            ("ctrl+shift+tab", "tab.switcher.prev", "shellWindow"),
            /*
             * Ctrl+` — the project switcher, on the key left of `1`.
             *
             * **`backquote`, never the literal tilde**, and this is the whole of why the request
             * for "ctrl+~" is not spelled the way it was asked for. `ui/src/keys/chords.ts`
             * derives a stroke from `KeyboardEvent.code` first, and that key is `Backquote` on
             * every layout whatever `key` reports — `` ` `` unshifted, `~` with Shift on a US
             * layout, something else elsewhere. `normalize_key` here deliberately renames no
             * keys, and the frontend's alias table folds `` ` ``, `tilde` and `grave` onto
             * `backquote`. So a binding written as the literal tilde would normalise to a key
             * that `strokeFromEvent` can never produce: silently inert, which is the exact
             * failure `chords.ts` exists to prevent. (`~` has been added to that alias table, so
             * a user who writes what the request said still gets what they meant.)
             *
             * Two more reasons the literal is wrong even if it worked: it requires Shift, so the
             * *whole walk* would be a Ctrl+Shift hold; and the capture reads Shift as the walk's
             * direction, so a shifted opener would collide with its own reverse.
             *
             * # What it costs, measured
             *
             * The pty: **nothing**. xterm encodes a control byte for plain Ctrl only for
             * keyCodes 65-90, 32, 51-55, 56, 219, 220 and 221; Backquote is 192 and is not in
             * that set. CodeMirror: nothing — no `Mod-` backquote binding exists in any of the
             * installed keymaps. The keymap: nothing — no default used the unshifted key.
             *
             * What it does cost is legibility, and it is written here rather than discovered:
             * Ctrl+` switches project while Ctrl+Shift+` splits a terminal below, which are
             * unrelated acts one Shift apart. And Ctrl+` is VS Code's toggle-terminal chord, so
             * somebody arriving from there will press it expecting a panel.
             *
             * `project.switcher.prev` gets no chord of its own: `ctrl+shift+backquote` is taken
             * by `terminal.splitBelow`, and moving a shipped binding nobody complained about to
             * make room is a worse trade than leaving the reverse to the palette — and to Shift
             * *during* the walk, which the capture claims for as long as the popup is up. That
             * shadows `terminal.splitBelow` for the few hundred milliseconds Ctrl is held, which
             * is the same trade `ctrl+shift+tab` has always made.
             */
            ("ctrl+`", "project.switcher.next", "shellWindow"),
            /*
             * Ctrl+1 — the pinned Claude console.
             *
             * Free in every layer, measured rather than assumed: no default binds any digit;
             * xterm's plain-Ctrl encode set does not contain keyCode 49, so no byte is taken
             * from any shell; and no `@codemirror` package binds a `Mod-` digit anywhere.
             *
             * **Ctrl+2..9 are deliberately not shipped.** They are the browser convention and
             * they read consistently with this one, and all three arguments against them are
             * real: the user asked for one chord and not a family; `tabs[1..]` are positional
             * and shift under the user as tabs open and close, where `tabs[0]` is an identity
             * Rust enforces; and Ctrl+3..8 are *not* free — they encode ESC, FS, GS, RS, US and
             * DEL — so the family would cost every terminal in every window six control codes
             * for a gesture nobody asked for. The precedents diverge too: IDEA uses Alt+1..9 for
             * tool windows and has no Ctrl+digit tab selection at all.
             */
            ("ctrl+1", "tab.console", "shellWindow"),
            /*
             * F4 — hide the left panel, or bring back the last one that was open.
             *
             * Free in every layer: `ctrl+f12` above is the only other f-key in this table,
             * CodeMirror binds no `F4` in any installed keymap, and Alt+F4 belongs to the
             * compositor while bare F4 is untouched by it.
             *
             * # What it costs a terminal, precisely
             *
             * xterm *encodes* F4: bare, it sends `ESC O S`, and modified `ESC [ 1 ; m S`. That
             * is a real key to `mc` (Edit), to `htop` (Filter) and to any curses TUI, and the
             * gate is a window capture listener — so this binding takes those bytes from every
             * pane in the shell window, unconditionally. The same trade `ctrl+t` makes above,
             * and the same escape hatch pays for it: one line of `keymap.json` naming `f4` with
             * a leading `-` on the command gives F4 back, and the `when` has to be repeated on
             * that line or the removal matches nothing.
             *
             * `when("!terminalFocused")` was the obvious alternative and it loses for the reason
             * `ctrl+t` states by name: it would make the hotkey dead exactly where a
             * terminal-centric app puts the user's hands. `shellWindow` is the clause that earns
             * its place — a detached-pane window has no rail and no sidebar, ever.
             *
             * macOS needs no exception: `ctrl_to_meta` only rewrites chords containing `ctrl`,
             * so a bare f-key passes through untouched.
             */
            ("f4", "sidebar.toggle", "shellWindow"),
            /*
             * Code folding — IDEA's chords, spelled the way this table has to spell them. (M19)
             *
             * # `minus` / `equal` / `plus`, never `-` or `+`
             *
             * `ui/src/keys/chords.ts::strokeFromEvent` reads `KeyboardEvent.code` **before**
             * `key`, and `CODE_NAMES` maps `Minus → minus`, `Equal → equal`,
             * `NumpadSubtract → minus`, `NumpadAdd → plus`. Two consequences, both load-bearing:
             * one `ctrl+minus` covers the main row *and* the numeric keypad, which is what makes
             * this IDEA-compatible on a full keyboard and on a laptop; and `equal` and `plus` are
             * two different physical keys, so "expand" needs both lines. A binding written
             * `ctrl+shift+-` would parse here — `normalize_key` renames no keys — and be inert
             * for ever, because no keystroke ever produces the token `-`.
             *
             * # Why the `when` is not optional
             *
             * The gate is a window **capture** listener, so an unscoped chord is taken from every
             * pane in every window. xterm encodes a control byte for a plain `Ctrl+-` (0x1f), so
             * an unscoped `ctrl+minus` would be a keystroke silently removed from every shell.
             * `editorFocused` is the same reasoning `ctrl+b` and `ctrl+g` above are scoped by.
             *
             * # What was checked, not assumed
             *
             * Nothing in this table binds `minus`, `equal`, `plus` or `period`; `defaultKeymap`,
             * `historyKeymap` and `closeBracketsKeymap` bind none of them either. On macOS the
             * blanket `ctrl_to_meta` rewrite lands these on ⌘−/⌘=/⌘. — which is what IDEA uses
             * there — and none of them is in `MACOS_MENU_CHORDS`, so AppKit's menu bar does not
             * eat them before the webview sees them.
             *
             * CodeMirror's own `foldKeymap` is deliberately *not* installed. It spells these
             * `Ctrl-Shift-[` / `Ctrl-Alt-[`, one modifier from `indentLess`/`indentMore`, and on
             * macOS `⌘[`/`⌘]` are already `navigate.back`/`navigate.forward` — pushed by
             * [`platform_layer`] below.
             */
            ("ctrl+minus", "editor.fold", "editorFocused"),
            ("ctrl+equal", "editor.unfold", "editorFocused"),
            ("ctrl+plus", "editor.unfold", "editorFocused"),
            ("ctrl+shift+minus", "editor.foldAll", "editorFocused"),
            ("ctrl+shift+equal", "editor.unfoldAll", "editorFocused"),
            ("ctrl+shift+plus", "editor.unfoldAll", "editorFocused"),
            ("ctrl+alt+minus", "editor.foldRecursively", "editorFocused"),
            (
                "ctrl+alt+equal",
                "editor.unfoldRecursively",
                "editorFocused",
            ),
            ("ctrl+alt+plus", "editor.unfoldRecursively", "editorFocused"),
            ("ctrl+period", "editor.toggleFold", "editorFocused"),
        ]
        .into_iter()
        .map(|(key, command, when)| Binding::new(key, command).when(when)),
    )
    .collect()
}

/// Platform corrections applied on top of [`defaults`].
///
/// Empty on Linux and Windows. On macOS every `ctrl` default is unbound and rebound under
/// `meta`, which is what the mock draws.
pub fn platform_defaults() -> Vec<Binding> {
    platform_layer(cfg!(target_os = "macos"))
}

/// The platform layer for a given platform, so both branches are testable on one machine.
fn platform_layer(macos: bool) -> Vec<Binding> {
    if !macos {
        return Vec::new();
    }
    let mut out = Vec::new();
    for binding in defaults() {
        if keeps_ctrl_on_macos(&binding.key) {
            continue;
        }
        let Some(meta_key) = ctrl_to_meta(&binding.key) else {
            continue;
        };
        // Unbind the ctrl spelling rather than leaving both alive: on macOS ctrl+p, ctrl+n
        // and friends are system text-navigation bindings, and shadowing them would make
        // every text field in the app behave unlike every other Mac app.
        out.push(Binding {
            key: binding.key.clone(),
            command: format!("-{}", binding.command),
            when: binding.when.clone(),
            args: None,
        });
        out.push(Binding {
            key: meta_key,
            command: binding.command,
            when: binding.when,
            args: binding.args,
        });
    }

    /*
     * ⌘[ and ⌘] — Back and Forward, added to the layer rather than produced by it. (M16)
     *
     * The mac chord for this in Xcode, in VS Code, in IDEA-on-mac and in every browser. On
     * Linux the thumb buttons are the whole of it (`mouseback`/`mouseforward` above), and those
     * carry no `ctrl`, so `ctrl_to_meta` returns `None` and the rewrite loop leaves them alone
     * on macOS too — `the_macos_layer_leaves_the_thumb_buttons_alone` asserts exactly that. So
     * this is an addition and cannot collide with a rewritten form of them.
     *
     * # Why not `("ctrl+[", …)` in `defaults()`, and let the loop above produce ⌘[ for free
     *
     * Because Ctrl+[ **is** the ESC character, 0x1b, on every terminal ever made, and the key
     * gate is a window *capture* listener: the binding would swallow Escape-equivalent in every
     * terminal pane in every window on Linux, which is the failure `ctrl+b`'s `editorFocused`
     * clause exists to prevent, without the clause. Ctrl+] is telnet's escape and vim's
     * jump-to-tag. Neither is free, so neither is written down.
     *
     * # Spelling
     *
     * `bracketleft`, not `[`. `normalize_key` deliberately renames no keys — the trap
     * `keeps_ctrl_on_macos` documents for backquote — so the Rust side would keep whichever was
     * written. The frontend folds them together either way (`chords.ts`'s `ALIASES` maps `[` to
     * `bracketleft` on the *binding string* as well as on the event, and `CODE_NAMES` maps the
     * `BracketLeft` code to the same name), and `chords.ts` renders it back as `[` for the
     * palette's chip. So the choice is about which spelling makes the two sides look identical
     * to somebody grepping, and that is this one.
     *
     * No `when`, matching `mouseback`/`mouseforward`: the one precondition that matters —
     * somewhere to go back to — cannot be a context flag and is re-checked in `keys/dispatch.ts`,
     * which reports the refusal as a sentence. What it costs a *terminal* is the point of the
     * paragraph above and is why the Linux spelling is not the same chord.
     *
     * # What it takes from CodeMirror, named rather than discovered
     *
     * `@codemirror/commands` binds `Mod-[`/`Mod-]` to `indentLess`/`indentMore`, and `Mod` is ⌘
     * on macOS — so those two chords go dead inside a buffer there. **The capability does not
     * go with them**: `indentWithTab` is in `EditorSurface`'s keymap, so Tab and Shift+Tab indent
     * and dedent the selection, which is what IDEA-on-mac binds them to as well.
     * `check-editor.mjs` pins that survival path by command identity rather than by key string.
     */
    out.push(Binding::new("meta+bracketleft", "navigate.back"));
    out.push(Binding::new("meta+bracketright", "navigate.forward"));

    out
}

/// The chords macOS's own menu bar takes before the web view is ever asked.
///
/// cide never calls `.menu()` and never calls `.enable_macos_default_menu(false)`, so on macOS
/// tauri installs `Menu::default` for it (`tauri-2.11.5` `src/app.rs`, in `build()`). AppKit
/// resolves a menu item's key equivalent in `performKeyEquivalent:` **before** the key reaches
/// WKWebView, so `ui/src/keys/gate.ts` — a window capture listener inside the document — never
/// sees these at all. A binding whose macOS spelling lands here is not merely shadowed; it is
/// unreachable, and it is still listed in Settings → Keymap looking perfectly healthy.
///
/// The list is the accelerators `muda-0.19.2`'s `PredefinedMenuItemType::accelerator` gives the
/// items `Menu::default` puts in, **as this module spells a chord** — modifiers in
/// `ctrl alt shift meta` order. `maximize` is in that menu and is deliberately absent here: it
/// is the one predefined item with no accelerator.
///
/// Two entries are worth knowing about even though nothing collides with them:
///
/// * **`meta+q`.** cide has no `app.quit` command at all — on Linux the window manager's close
///   button and `RunEvent::ExitRequested` are the whole of it — so ⌘Q *is* this menu item, and
///   the quit path on a Mac is a gesture cide does not own. `cide_app::lib`'s run loop now
///   handles `RunEvent::Exit` for exactly that reason.
/// * **`meta+c` / `meta+v` / `meta+x` / `meta+a` / `meta+z`.** These are the Edit submenu, and
///   they are the argument *against* the obvious fix of turning the default menu off: on macOS
///   a menu item's `copy:`/`paste:` is a large part of how clipboard keys reach a text view at
///   all, and finding out whether WKWebView still handles them unaided needs a Mac.
pub const MACOS_MENU_CHORDS: [(&str, &str); 12] = [
    ("meta+c", "Edit → Copy"),
    ("meta+x", "Edit → Cut"),
    ("meta+v", "Edit → Paste"),
    ("meta+z", "Edit → Undo"),
    ("shift+meta+z", "Edit → Redo"),
    ("meta+a", "Edit → Select All"),
    ("meta+m", "Window → Minimize"),
    ("meta+w", "File → Close Window"),
    ("meta+q", "cide → Quit"),
    ("meta+h", "cide → Hide"),
    ("alt+meta+h", "cide → Hide Others"),
    ("ctrl+meta+f", "View → Toggle Full Screen"),
];

/// A default binding whose macOS spelling is claimed by [`MACOS_MENU_CHORDS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacMenuConflict {
    /// The chord, as the macOS layer resolves it.
    pub key: String,
    /// The command that will never run for it there.
    pub command: String,
    /// The menu item that takes it instead.
    pub menu_item: &'static str,
}

/// Every default binding that macOS's menu bar makes unreachable.
///
/// Computed rather than listed, over the *rebinds* the platform layer produces — an unbind
/// (`-command`) is not a conflict, it is the layer removing a chord on purpose.
///
/// This is a fact about the product, not a lint, so it has a surface as well as a gate:
/// `cide-headless keymap` prints it under every dump on every host — the tool exists to inspect
/// the keymap without a window, and *"this binding cannot fire on a Mac"* is exactly the kind of
/// thing a window would never show you. `every_macos_menu_conflict_is_one_somebody_decided_to_keep`
/// fails when the set changes in either direction; growing it silently is how a keymap entry
/// becomes decoration.
pub fn macos_menu_conflicts() -> Vec<MacMenuConflict> {
    conflicts_in(platform_layer(true))
}

/// [`macos_menu_conflicts`] over a layer handed in, so the unbind rule below has a test.
///
/// The unbind filter is not decoration. A default already spelled with **both** modifiers —
/// `ctrl+meta+f` is the shape, and it is in the claimed list because ⌃⌘F is Full Screen — would
/// have that exact key on its `-command` line, and reporting a removal as a dead binding would
/// put a permanent false entry in the list this module exists to keep true.
fn conflicts_in(layer: Vec<Binding>) -> Vec<MacMenuConflict> {
    layer
        .into_iter()
        .filter(|binding| !binding.command.starts_with('-'))
        .filter_map(|binding| {
            MACOS_MENU_CHORDS
                .iter()
                .find(|(chord, _)| *chord == binding.key)
                .map(|(_, menu_item)| MacMenuConflict {
                    key: binding.key,
                    command: binding.command,
                    menu_item,
                })
        })
        .collect()
}

/// Chords that stay on `ctrl` even on macOS.
///
/// The blanket ctrl→meta rewrite is right for `⌘P`, `⌘S` and the rest, and wrong for exactly
/// two keys, both for the same reason: **macOS eats the ⌘ spelling itself**, so rewriting
/// would not move the binding, it would delete it.
///
/// * **Tab.** `⌘⇥` is the system application switcher, consumed before any app sees it. Ctrl+Tab
///   is what switches tabs on macOS in Safari, Chrome and VS Code anyway.
/// * **Backquote.** `⌘\`` is *move focus to the next window in this application* and `⇧⌘\`` the
///   previous one — a system shortcut in the same class. That covers both defaults on this key:
///   `project.switcher.next` and `terminal.splitBelow`.
///
/// This is the whole exception list; it is a function rather than a `const` array so the rule is
/// stated where the reason is. The key names are matched as `parse_chord` produces them, which
/// means the *written* spelling: `normalize_key` deliberately renames no keys, so a binding
/// written with a literal backtick has key `` ` `` and one written `backquote` has key
/// `backquote`. Both are listed rather than one of them being assumed, because the table above
/// uses the literal and a rewrite that missed it would be silent.
fn keeps_ctrl_on_macos(key: &str) -> bool {
    parse_chord(key)
        .map(|chords| {
            chords
                .iter()
                .any(|chord| matches!(chord.key.as_str(), "tab" | "`" | "backquote"))
        })
        .unwrap_or(false)
}

/// The same keystroke with `ctrl` moved to `meta`, or `None` when it uses no `ctrl` and so
/// needs no rewriting.
fn ctrl_to_meta(key: &str) -> Option<String> {
    let mut chords = parse_chord(key).ok()?;
    if !chords.iter().any(|chord| chord.ctrl) {
        return None;
    }
    for chord in &mut chords {
        if chord.ctrl {
            chord.ctrl = false;
            chord.meta = true;
        }
    }
    Some(render(&chords))
}

/// Layers `defaults` → `platform_defaults` → `user` into the keymap the frontend runs.
///
/// Keys in the result are normalised, so the frontend can match on them directly.
pub fn resolve(user: &[Binding]) -> Vec<ResolvedBinding> {
    resolve_layers(&platform_defaults(), user)
}

/// [`resolve`] with the platform layer injected, so tests can exercise the macOS layer on
/// any host.
fn resolve_layers(platform: &[Binding], user: &[Binding]) -> Vec<ResolvedBinding> {
    resolve_layers_reporting(platform, user).0
}

/// Something wrong with a binding that resolution recovered from rather than rejecting.
///
/// Resolution deliberately never fails: one bad line in `keymap.json` must not cost the
/// user every other binding in the file. But recovering silently is how this module's
/// stated failure mode — "my keybinding does nothing, with no visible cause" — happens, so
/// each recovery is reported instead.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum KeymapDiagnostic {
    /// The key could not be parsed, so it was folded to a best-effort string that no
    /// keystroke will ever produce.
    UnparseableKey {
        key: String,
        command: String,
        reason: String,
    },
    /// A `-command` entry matched nothing. Almost always a `when` clause that does not
    /// match the binding being targeted, which is the sharp edge of VS Code's removal
    /// semantics and worth saying out loud.
    RemovalMatchedNothing { key: String, command: String },
}

/// Resolve, and report every binding that had to be recovered from.
///
/// [`resolve`] is this with the diagnostics dropped; the split exists so the Settings →
/// Keymap screen can show a user what in their file is not doing what they think.
pub fn resolve_with_diagnostics(user: &[Binding]) -> (Vec<ResolvedBinding>, Vec<KeymapDiagnostic>) {
    resolve_layers_reporting(&platform_defaults(), user)
}

fn resolve_layers_reporting(
    platform: &[Binding],
    user: &[Binding],
) -> (Vec<ResolvedBinding>, Vec<KeymapDiagnostic>) {
    let mut out = Vec::new();
    let mut diags = Vec::new();
    // The compiled-in layers are covered by a test that every default parses, so a
    // diagnostic from them would be a bug in this crate rather than in the user's file.
    apply_layer(&mut out, KeymapLayer::Default, &defaults(), &mut diags);
    apply_layer(&mut out, KeymapLayer::Platform, platform, &mut diags);
    apply_layer(&mut out, KeymapLayer::User, user, &mut diags);
    (out, diags)
}

/// Folds one layer into the accumulated result.
fn apply_layer(
    out: &mut Vec<ResolvedBinding>,
    layer: KeymapLayer,
    bindings: &[Binding],
    diags: &mut Vec<KeymapDiagnostic>,
) {
    // This layer's own entries are held aside and appended at the end, so that a binding
    // cannot shadow another binding from the same layer. Within-layer duplicates are a
    // mistake worth surfacing through `conflicts`, not something to resolve quietly.
    let mut added: Vec<ResolvedBinding> = Vec::new();

    for binding in bindings {
        // Folding an unparseable key is the recovery; reporting it is what stops the
        // recovery from being indistinguishable from success.
        if let Err(e) = parse_chord(&binding.key) {
            diags.push(KeymapDiagnostic::UnparseableKey {
                key: binding.key.clone(),
                command: binding.command.clone(),
                reason: e.to_string(),
            });
        }
        let key = normalize_key(&binding.key);

        let when = normalize_when(binding.when.as_deref());

        if binding.is_removal() {
            // Removal matches on (key, command, when), which is what VS Code does and what
            // the format this module implements documents — see the `-tab.close` example in
            // `cide-ipc::keymap`, whose whole point is a removal scoped to one context.
            //
            // The strictness is real: a removal that omits `when` only removes bindings that
            // also have none, so a user who forgets it sees nothing happen. That is why a
            // removal matching nothing is reported through `resolve_with_diagnostics`
            // rather than swallowed.
            let command = binding.target_command();
            let matches =
                |r: &ResolvedBinding| r.key == key && r.command == command && r.when == when;
            let before = out.len() + added.len();
            out.retain(|r| !matches(r));
            added.retain(|r| !matches(r));
            if out.len() + added.len() == before {
                diags.push(KeymapDiagnostic::RemovalMatchedNothing {
                    key,
                    command: command.to_string(),
                });
            }
            continue;
        }

        // Same keystroke and same context as an earlier layer means an override; a different
        // `when` means the two coexist, which is how one key means different things in the
        // terminal and in the editor.
        out.retain(|r| !(r.key == key && r.when == when));
        added.push(ResolvedBinding {
            key,
            command: binding.command.clone(),
            when,
            args: binding.args.clone(),
            layer,
        });
    }

    out.append(&mut added);
}

/// Reads `keymap.json`. A missing file means "no overrides", not an error.
///
/// The file holds overrides only, so its absence is the default state of a fresh install and
/// must not surface as a failure anywhere up the stack.
pub fn load_user(path: &Path) -> Result<Vec<Binding>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    // An empty file is what `touch` leaves behind, and reporting a JSON parse error for it
    // would be a confusing way to say "you have written nothing yet".
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&text)?)
}

// --- editing the user layer ----------------------------------------------------------------
//
// Everything below turns a gesture on the Settings → Keymap screen into entries in
// `keymap.json`. It is here rather than in `cide-app` for the reason the whole crate exists:
// it is arithmetic over a `Vec<Binding>`, it needs no filesystem and no `AppHandle`, and the
// traps it has to avoid are already pinned by tests a few hundred lines down.
//
// # The three rules that decide every function here
//
// 1. **The file is a diff.** Defaults are compiled in, so an edit writes what the user
//    *changed* and never what the resolved table currently says. A screen that saved its own
//    contents would freeze today's defaults into the user's file for ever.
// 2. **One gesture is usually two entries.** Moving a command off a default chord needs a
//    removal on the old key and an add on the new one; leaving the removal out binds the
//    command twice, and `conflicts` then reports a conflict the editor itself manufactured.
// 3. **A removal matches on (key, command, `when`) — all three.** So the `when` written into a
//    removal has to be the one the binding being removed actually carries. It is copied from
//    the resolved binding rather than reconstructed, because a reconstructed one that differs
//    by a word matches nothing, changes nothing, and reports `RemovalMatchedNothing` to a user
//    who is looking at a screen that says the key is now free.
//
// The fourth rule is a consequence of the first three: **strip before you append**. Within a
// layer duplicates deliberately coexist (see the module note), so appending an override on top
// of the user's own earlier override leaves both alive and produces a `Conflict`. Every editor
// below therefore removes the user entries it supersedes first, and only then works out what
// the layers underneath still need suppressing.

/// What one [`apply_edit`] did to the user layer.
///
/// Counted rather than inferred from the file's length, because the interesting outcome is the
/// one where nothing happened: a *Restore default* that matched no entry — the `when` trap
/// above, arriving through the reset door — looks exactly like a successful one on screen
/// unless the answer says `removed: 0`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditOutcome {
    /// Entries deleted from the user's file.
    pub removed: usize,
    /// Entries appended to it, removals included.
    pub added: usize,
}

/// Apply one edit to a user layer, in place.
///
/// Pure: `user` goes in as the file's parsed contents and comes out as what should be written
/// back. Nothing here touches the disk, so the whole surgery is testable without one — the
/// same split `cide_app::cmd::settings::apply_patch` already makes.
pub fn apply_edit(user: &mut Vec<Binding>, edit: &KeymapEdit) -> EditOutcome {
    apply_edit_layered(&platform_defaults(), user, edit)
}

/// [`apply_edit`] with the platform layer injected, the exact counterpart of [`resolve_layers`].
///
/// An edit is not platform-independent even though it looks it: every branch below asks what is
/// *standing* underneath the user's file, and on macOS the answer has been rewritten by
/// [`platform_layer`] — `picker.files` is on `meta+p` there, not `ctrl+p`. So a rebind writes a
/// removal naming a different chord, and a command the macOS layer gives a second chord to needs
/// two removal lines rather than one.
///
/// This seam exists for the reason `resolve_layers` does, and it was added for a concrete one:
/// sixteen tests below drove `apply_edit`/`resolve` and asserted a literal `ctrl+…` spelling, so
/// they passed on Linux and failed on macOS — against an editor that was behaving *correctly for
/// macOS*. Naming the layer under test makes each of them assert one platform's rule on every
/// host, which is what a unit test of the editing algebra should do; the platform layers keep
/// their own dedicated tests. Nothing here consults `cfg!` — only [`platform_defaults`] does.
fn apply_edit_layered(
    platform: &[Binding],
    user: &mut Vec<Binding>,
    edit: &KeymapEdit,
) -> EditOutcome {
    match edit {
        KeymapEdit::ResetAll => {
            let removed = user.len();
            user.clear();
            EditOutcome { removed, added: 0 }
        }
        KeymapEdit::Reset { command, when } => EditOutcome {
            removed: strip(user, command, when.as_deref()),
            added: 0,
        },
        KeymapEdit::Unbind { command, when } => {
            let removed = strip(user, command, when.as_deref());
            // Nothing kept: every key the layers below still put this command on has to be
            // taken away, or "unbound" would mean "unbound unless a default says otherwise".
            let added = suppress(platform, user, command, when.as_deref(), None);
            EditOutcome { removed, added }
        }
        KeymapEdit::Rebind { command, when, key } => {
            // Taking, not just counting: the entry being replaced may carry `args`, and the
            // replacement below has to carry them forward. See `strip_taking`.
            let taken = strip_taking(user, command, when.as_deref());
            let removed = taken.len();
            let args = taken.into_iter().find_map(|binding| binding.args);
            let target = normalize_key(key);
            let mut added = suppress(platform, user, command, when.as_deref(), Some(&target));

            // Asked of the resolution rather than of `defaults()`, and after the removals have
            // been appended, so this reads the table as it will actually be. When the answer is
            // yes the file gets no entry at all: rebinding a command back onto its own default
            // chord *is* a reset, and writing the default in would freeze it.
            let when = normalize_when(when.as_deref());
            let standing = resolve_layers(platform, user)
                .into_iter()
                .any(|r| r.key == target && r.command == *command && r.when == when);
            if !standing {
                user.push(Binding {
                    key: target,
                    command: command.clone(),
                    when,
                    args,
                });
                added += 1;
            }
            EditOutcome { removed, added }
        }
    }
}

/// May this batch of edits be written over a `keymap.json` that could not be parsed?
///
/// Only **Reset all**, on its own. A file with a stray comma — or with the `//` comment a user
/// assumed was legal, this being VS Code's *shape* and not its JSONC parser — reads as *no
/// overrides*, so applying an ordinary edit to it and saving would replace everything the user
/// had written with the one line they just recorded, and report success. Refusing is the only
/// honest answer, and *Reset all* is the exception because it is the one gesture that already
/// means "throw away what is in there" — which also makes it the way out for a user who would
/// rather not go and find the file.
///
/// A predicate rather than three lines inside the command handler, because a rule that lives
/// in a handler is a rule no test can reach.
pub fn may_replace_unreadable(edits: &[KeymapEdit]) -> bool {
    edits == [KeymapEdit::ResetAll]
}

/// Delete every user entry aimed at (`command`, `when`) — adds and removals alike.
///
/// Matches on the *target* command, so the `-picker.files` removal an earlier rebind wrote is
/// dropped alongside the `picker.files` add that came with it. That pairing is why *Restore
/// default* is a deletion and not a write: the two entries together are what a rebind is, and
/// removing both is what lets the compiled-in default reappear underneath.
fn strip(user: &mut Vec<Binding>, command: &str, when: Option<&str>) -> usize {
    strip_taking(user, command, when).len()
}

/// [`strip`], but hands back what it removed.
///
/// Exists so a rebind can **carry the old entry's `args` forward**. `strip` deletes the whole
/// binding and the replacement used to be pushed with `args: None`, so a hand-authored
/// `{"key":"ctrl+alt+o","command":"file.reveal","args":{"path":"…"}}` — a shape `dispatch.ts`
/// documents by name and really does honour — came back on the new chord with its payload gone.
/// The binding still existed and quietly did something else, which is worse than losing it: the
/// user sees their command on the key they chose and no sign that half of it was dropped.
fn strip_taking(user: &mut Vec<Binding>, command: &str, when: Option<&str>) -> Vec<Binding> {
    let when = normalize_when(when);
    let mut taken = Vec::new();
    user.retain(|binding| {
        let hit =
            binding.target_command() == command && normalize_when(binding.when.as_deref()) == when;
        if hit {
            taken.push(binding.clone());
        }
        !hit
    });
    taken
}

/// Append the removals that take (`command`, `when`) off every key the layers below still put
/// it on, except `keep`.
///
/// Call **after** [`strip`], never before: it asks `resolve_layers` what is standing, and a user
/// entry for the same command that has not been stripped yet would answer for the defaults.
///
/// The key each removal names is the one `resolve` reports, which is already normalised — the
/// third rule at the top of this section, applied to the other half of the identity. Writing
/// the key the *caller* typed would work for every chord a user spells canonically and fail
/// silently for the ones they do not.
fn suppress(
    platform: &[Binding],
    user: &mut Vec<Binding>,
    command: &str,
    when: Option<&str>,
    keep: Option<&str>,
) -> usize {
    let when = normalize_when(when);
    let mut keys: Vec<String> = Vec::new();
    for binding in resolve_layers(platform, user) {
        if binding.command != command || binding.when != when {
            continue;
        }
        if Some(binding.key.as_str()) == keep || keys.contains(&binding.key) {
            continue;
        }
        keys.push(binding.key);
    }

    let added = keys.len();
    for key in keys {
        user.push(Binding {
            key,
            command: format!("-{command}"),
            when: when.clone(),
            args: None,
        });
    }
    added
}

/// Write the user layer to `path`, keeping the previous contents beside it as `.bak`.
///
/// # What this does to a file somebody wrote by hand
///
/// It reflows it. The vector is re-serialised with `to_vec_pretty`, so indentation, line
/// breaks and key order inside each entry become serde's rather than the author's, and any
/// field `Binding` does not know about — a `"note"` a user added to remind themselves why —
/// is gone, because it was never parsed. Entry *order* survives, and has to: `apply_layer`
/// walks the file in sequence and a removal only matches what is already accumulated, so
/// rebuilding the file from a sorted or regrouped copy would change what it means.
///
/// The `.bak` is the honest answer to the reflow rather than an apology for it. It is written
/// before every save, not once, so the copy is always the state immediately before the last
/// edit — the thing a user actually wants back when a click did something they did not expect.
/// Through [`persist::write_atomic`] as well, so the backup gets the same 0600 and the same
/// crash safety as the file it is protecting.
///
/// # Two things it changes about the file itself, both deliberate
///
/// * **The mode becomes 0600.** `write_atomic` publishes by renaming a private temp file over
///   the target, and the mode travels with the inode. For a file that may hold nothing more
///   secret than `ctrl+t` that is stricter than it needs to be; it is also the one part of
///   that function nobody should be re-deciding per call site, which is exactly why it is
///   `pub` (read its doc comment for the 0644 leak that made it so).
/// * **A symlink is followed, not replaced.** `keymap.json` symlinked into a dotfiles
///   repository is how a hand-authored one usually gets onto a machine, and a plain rename
///   would quietly replace the link with a regular file and strand the repository copy. So an
///   existing path is canonicalised first and the write lands on the real file. Canonicalising
///   only when the file exists, because the fresh-install case is precisely the one where
///   there is nothing to resolve.
pub fn save_user(path: &Path, bindings: &[Binding]) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    if let Ok(previous) = std::fs::read(&target) {
        // A failed backup is not a reason to refuse the edit — the user asked for a keybinding,
        // not for a backup — but it is worth a line, because the next paragraph of the UI
        // promises the file is there.
        if let Err(error) = persist::write_atomic(&backup_path(&target), &previous) {
            tracing::warn!(%error, "keymap.json backup could not be written");
        }
    }

    let mut json = serde_json::to_vec_pretty(bindings)?;
    // A trailing newline, because this is a file a user reads and edits in the editor of their
    // choice, and half of them will append to it.
    json.push(b'\n');
    persist::write_atomic(&target, &json)
}

/// `keymap.json` → `keymap.json.bak`, beside it.
fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Groups resolved bindings that fight over the same keystroke in the same context.
///
/// Bindings whose `when` clauses differ are not in conflict; that is the mechanism by which
/// one key means different things in different panes.
pub fn conflicts(resolved: &[ResolvedBinding]) -> Vec<Conflict> {
    // Insertion-ordered so the report is stable and the last command in each group is the
    // one that actually wins.
    let mut groups: IndexMap<(String, Option<String>), Vec<String>> = IndexMap::new();
    for binding in resolved {
        let commands = groups
            .entry((normalize_key(&binding.key), binding.when.clone()))
            .or_default();
        // The same command bound twice on one key is redundant, not contested.
        if !commands.contains(&binding.command) {
            commands.push(binding.command.clone());
        }
    }

    // An UNSCOPED binding applies everywhere, so it contests every scoped binding on the same
    // key rather than sitting in a group beside it.
    //
    // Grouping by `(key, when)` alone cannot see that: `when: None` and `when: Some("shellWindow")`
    // are two different tuples, so a user who bound a command to F4 with no context got no
    // conflict reported at all — while the gate's last-applicable-wins quietly took the panel
    // toggle away. The keymap editor exists to prevent exactly that, and it was the one collision
    // it could not see.
    //
    // Folded in afterwards rather than by widening the key, because the scoped groups are still
    // the right *report*: the user wants to be told which context they are contesting.
    let unscoped: Vec<(String, Vec<String>)> = groups
        .iter()
        .filter(|((_, when), _)| when.is_none())
        .map(|((key, _), commands)| (key.clone(), commands.clone()))
        .collect();
    for (key, commands) in unscoped {
        for ((other_key, when), scoped) in groups.iter_mut() {
            if when.is_none() || *other_key != key {
                continue;
            }
            for command in &commands {
                if !scoped.contains(command) {
                    scoped.push(command.clone());
                }
            }
        }
    }

    groups
        .into_iter()
        .filter(|(_, commands)| commands.len() > 1)
        .map(|((key, when), commands)| Conflict {
            key,
            commands,
            when,
        })
        .collect()
}

/// Canonical spelling of a key string: lowercase, modifiers in `ctrl alt shift meta` order,
/// one space between the strokes of a sequence.
///
/// Total by design — it is used as a map key and as an equality test, so it has to return
/// something for every input. A string that does not parse normalises to a whitespace- and
/// case-folded copy of itself, which compares equal to other spellings of the same broken
/// input and to nothing else.
pub fn normalize_key(key: &str) -> String {
    match parse_chord(key) {
        Ok(chords) => render(&chords),
        Err(_) => key
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    }
}

/// Canonical form of a `when` clause, so two spellings of one context compare equal.
///
/// Keys are normalised but `when` clauses used to be compared byte-exact, which meant an
/// override whose context differed only by surrounding whitespace failed to shadow the
/// binding it was aimed at — the same silent-override failure key normalisation exists to
/// prevent, arriving through the other half of the identity.
///
/// This trims and collapses internal runs of whitespace. It deliberately does not try to
/// understand the expression: `a && b` and `b && a` are left distinct, because deciding
/// they are the same needs a parser and an ordering, and getting *that* subtly wrong would
/// silently merge two bindings that the user meant to keep apart.
fn normalize_when(when: Option<&str>) -> Option<String> {
    let text = when?.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

/// Parses a key string into one [`Chord`] per stroke.
///
/// Malformed input is [`CoreError::Serde`]: this parses user-supplied data out of a config
/// file, so a bad string is a data error to report, never a panic.
pub fn parse_chord(key: &str) -> Result<Vec<Chord>> {
    let chords = key
        .split_whitespace()
        .map(parse_stroke)
        .collect::<Result<Vec<_>>>()?;
    if chords.is_empty() {
        return Err(CoreError::Serde("keybinding has an empty key".into()));
    }
    Ok(chords)
}

/// Parses one stroke, e.g. `ctrl+shift+p`.
fn parse_stroke(stroke: &str) -> Result<Chord> {
    let lowered = stroke.to_lowercase();

    // `+` is bindable and is spelled by writing it where the key goes, which makes the last
    // segment of a naive split empty. Take it literally instead of rejecting the stroke.
    //
    // `None` means the stroke carried no separator at all, so there is nothing to validate.
    // An empty `Some` is a stray leading separator and must fail below: `+p` is not `p`, and
    // silently treating it as such binds a bare letter the user never asked for.
    let (modifiers, key) = if lowered == "+" || lowered == "++" {
        (None, "+")
    } else if let Some(rest) = lowered.strip_suffix("++") {
        (Some(rest), "+")
    } else if let Some((modifiers, key)) = lowered.rsplit_once('+') {
        (Some(modifiers), key)
    } else {
        (None, lowered.as_str())
    };

    if key.is_empty() {
        return Err(CoreError::Serde(format!(
            "keybinding `{stroke}` has modifiers but no key"
        )));
    }

    let mut chord = Chord {
        key: key.to_owned(),
        ..Chord::default()
    };
    let Some(modifiers) = modifiers else {
        return Ok(chord);
    };
    for modifier in modifiers.split('+') {
        match modifier {
            "ctrl" | "control" => chord.ctrl = true,
            "alt" | "option" => chord.alt = true,
            "shift" => chord.shift = true,
            "meta" | "cmd" | "command" | "super" | "win" => chord.meta = true,
            // Reached by a doubled or leading separator such as `ctrl++h` or `+p`. Named
            // separately because "unknown modifier ``" reads as a bug in the parser rather
            // than a typo in the file.
            "" => {
                return Err(CoreError::Serde(format!(
                    "keybinding `{stroke}` has an empty modifier"
                )));
            }
            other => {
                return Err(CoreError::Serde(format!(
                    "keybinding `{stroke}` has an unknown modifier `{other}`"
                )));
            }
        }
    }
    Ok(chord)
}

/// Joins chords with the single space that separates strokes of a sequence.
fn render(chords: &[Chord]) -> String {
    chords
        .iter()
        .map(Chord::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that removes itself, so a failing assertion cannot leave litter behind.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("cide-keymap-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn write(&self, name: &str, contents: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents).expect("write temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn command_for(resolved: &[ResolvedBinding], key: &str) -> Vec<String> {
        let key = normalize_key(key);
        resolved
            .iter()
            .filter(|r| r.key == key)
            .map(|r| r.command.clone())
            .collect()
    }

    /* ---- Naming the platform layer under test -------------------------------------------
     *
     * `resolve` and `apply_edit` consult `platform_defaults()`, which is the module's only
     * `cfg!`. Every test that asserts a *literal chord spelling* therefore asserts a platform,
     * whether or not it means to — and the ones below meant to test the layering and editing
     * algebra, using `defaults()` merely as a fixture. On macOS `platform_layer` moves every
     * `ctrl` default onto `meta`, so sixteen of them failed there against an editor that was
     * doing exactly the right thing. That is a test bug, not a product bug, and gating them off
     * macOS would have been the wrong repair twice over: it deletes the coverage on the platform
     * that has the least, and it leaves the remaining assertions passing *vacuously*.
     *
     * So the layer is named instead. `resolve_pc`/`edit_pc` pin the PC layer — empty, as Linux
     * and Windows both get — so these tests assert one platform's rule identically on every
     * host. The macOS layer is not thereby untested: it has `the_macos_layer_*` tests of its
     * own, and `an_unbind_on_macos_takes_away_both_chords_the_layer_gave` below covers the one
     * place where the editing algebra genuinely differs between the two.
     */

    /// [`resolve`] against the PC platform layer, whatever host this is compiled on.
    fn resolve_pc(user: &[Binding]) -> Vec<ResolvedBinding> {
        resolve_layers(&platform_layer(false), user)
    }

    /// [`apply_edit`] against the PC platform layer, whatever host this is compiled on.
    fn edit_pc(user: &mut Vec<Binding>, edit: &KeymapEdit) -> EditOutcome {
        apply_edit_layered(&platform_layer(false), user, edit)
    }

    /// [`resolve_with_diagnostics`] against the PC platform layer, ditto.
    fn resolve_with_diagnostics_pc(
        user: &[Binding],
    ) -> (Vec<ResolvedBinding>, Vec<KeymapDiagnostic>) {
        resolve_layers_reporting(&platform_layer(false), user)
    }

    /// **Ctrl+C and Ctrl+V are not the keymap's, and must never become the keymap's.**
    ///
    /// They are resolved inside the terminal instead — `ui/src/terminal/keys.ts`, from xterm's
    /// own custom key handler — and the difference is not stylistic. The key gate's second entry
    /// point is a window **capture** listener, so a binding here is consumed before the event
    /// reaches its target: it would take Ctrl+C and Ctrl+V away from the file tree (which has
    /// its own copy/cut/paste over the selected rows), from the commit message box, from every
    /// rename field and from every text input in the app. A `when: "terminalFocused"` clause
    /// does not save it — that flag is derived from `tab.tree.focused`, which stays true while
    /// the caret is in any of those.
    ///
    /// And Ctrl+C is the interrupt. A binding here swallows the keystroke, so the pty never
    /// receives `ETX` and a runaway command or a mid-turn `claude` cannot be stopped from the
    /// keyboard at all. That is the regression this test exists to make loud, because "just add
    /// a binding" is the obvious-looking way to implement copy and it is silently catastrophic.
    #[test]
    fn the_terminal_clipboard_chords_are_not_bound_here() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            assert!(
                key != "ctrl+c" && key != "ctrl+v",
                "{key} is bound to {} — the terminal owns these two, focus-scoped; see \
                 ui/src/terminal/keys.ts",
                binding.command
            );
        }
    }

    /// **Undo and redo are CodeMirror's, and must never become the keymap's.**
    ///
    /// The same argument as [`the_terminal_clipboard_chords_are_not_bound_here`] above, arriving
    /// through a different door. `@codemirror/commands`' `historyKeymap` already binds `Mod-z`,
    /// `Mod-y`, `Ctrl-Shift-z` (its Linux spelling for redo), `Mod-u` and `Alt-u`, and
    /// `EditorSurface` installs it — so all of these work in a buffer today *because* this table
    /// is silent about them.
    ///
    /// Adding one here would not add a capability, it would remove five. The key gate's second
    /// entry point is a window **capture** listener (`ui/src/keys/gate.ts`), so a binding is
    /// consumed before the event reaches its target: Ctrl+Z would stop reaching CodeMirror, the
    /// find field beside it, the commit message box, every rename field in the file tree and
    /// every plain `<input>` in the app, all of which have working native undo precisely because
    /// nothing intercepts it. And in a terminal pane Ctrl+Z is **SIGTSTP** — a binding here
    /// silently removes the only keyboard way to suspend a foreground job.
    ///
    /// A `when: "editorFocused"` clause does not rescue it. That flag is derived from the focused
    /// *pane kind* (`ui/src/keys/context.ts`), so it stays true while the caret sits in the find
    /// bar inside that pane — which is one of the inputs the binding would break.
    ///
    /// `ui/scripts/check-key-gate.mjs` proves the other half, that the gate passes these strokes
    /// through at both entry points in every context.
    #[test]
    fn the_editor_undo_chords_are_not_bound_here() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            assert!(
                !matches!(
                    key.as_str(),
                    "ctrl+z" | "ctrl+y" | "ctrl+shift+z" | "ctrl+u" | "alt+u"
                ),
                "{key} is bound to {} — undo/redo belong to CodeMirror's historyKeymap, and a \
                 binding here is swallowed by the window capture gate before any text surface \
                 sees it (ctrl+z in a terminal is SIGTSTP)",
                binding.command
            );
        }
    }

    /// **F3 and Shift+F3 stay unbound, because they are the find bar's only surviving chord.**
    ///
    /// `@codemirror/search`'s `searchKeymap` binds `F3`/`Shift-F3` to find-next/find-previous
    /// with `scope: "editor search-panel"`, and `ui/src/editor/find.ts` installs it whole and
    /// bridges the panel's own keydown into that scope. A binding here would be consumed by the
    /// window capture gate in *both* contexts at once — the buffer and the find field — and the
    /// bar would be left with Enter and the two arrow buttons.
    ///
    /// This matters more since `ctrl+g` became `navigate.line` below. `searchKeymap` also binds
    /// `Mod-g` to find-next, and that spelling is now swallowed inside an editor, so F3 is not a
    /// convenience — it is what find-next has left. The comment on the `ctrl+g` binding states
    /// the same trade from the other end; this is the half a compiler can check.
    #[test]
    fn nothing_binds_the_find_bars_f_keys() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            assert!(
                key != "f3" && key != "shift+f3",
                "{key} is bound to {} — F3 is find-next inside the editor, and the window \
                 capture gate would take it from the buffer and the find field together",
                binding.command
            );
        }
    }

    /// **Tab and Ctrl+Space are not bound here, and for the fourth version of the argument.** (M25)
    ///
    /// Both belong to the completion popup (`ui/src/editor/completion.ts`), and both are claimed
    /// there as ordinary CodeMirror bindings rather than as commands — exactly as `Mod-f`, `F3`
    /// and `Escape` are, and for the reason [`nothing_binds_the_find_bars_f_keys`] gives: the key
    /// gate's second entry point is a window **capture** listener, so a binding in this table is
    /// resolved *before* the event reaches the DOM and CodeMirror is never offered the stroke.
    ///
    /// What each one would cost, because they are not the same loss:
    ///
    /// * **Tab.** The stroke does three things depending on context — accept a completion, move
    ///   to the next snippet field, indent — and only the surface with the caret in it knows
    ///   which. A keymap entry here would take all three, in every editor, permanently. It would
    ///   also take Tab from every text input in the window, since a `when` clause cannot help:
    ///   `editorFocused` is derived from the focused *pane*, not from the caret, and stays true
    ///   while somebody types in a rename field or the commit box.
    /// * **Ctrl+Space.** It is a terminal's `NUL` (`^@`, set-mark in readline and Emacs), so
    ///   binding it here would swallow it in every shell pane in the application to give the
    ///   editor a chord it already has.
    ///
    /// A user who wants either as a command can still write one line of `keymap.json`. What must
    /// not happen is cide shipping that line.
    #[test]
    fn nothing_binds_the_editors_completion_chords() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            assert!(
                key != "tab" && key != "shift+tab",
                "{key} is bound to {} — Tab accepts a completion and walks snippet fields inside \
                 the editor, and the window capture gate would take it from the buffer and from \
                 every text input in the window at once",
                binding.command
            );
            assert!(
                key != "ctrl+space",
                "{key} is bound to {} — Ctrl+Space opens the completion popup inside the editor, \
                 and binding it here would also swallow a terminal's NUL in every shell pane",
                binding.command
            );
        }
    }

    /// **Ctrl+F is not bound here either, and for the third version of the same argument.**
    ///
    /// It means two different things in two different panes, and the keymap can express neither
    /// without breaking the other. In an editor it is `@codemirror/search`'s `Mod-f`, which opens
    /// the find bar and is reached *only* because this table stays silent about the key —
    /// `ui/src/editor/EditorSurface.tsx` says so where it fixed the read-only-buffer keymap. In a
    /// terminal it opens that pane's find bar, and that is resolved focus-scoped in
    /// `ui/src/terminal/keys.ts` from xterm's own custom key handler, beside Ctrl+C and Ctrl+V.
    ///
    /// A `when: "terminalFocused"` binding is the shape that looks like it would work and does
    /// not, for the reason [`the_terminal_clipboard_chords_are_not_bound_here`] spells out: the
    /// gate's second entry point is a window **capture** listener and that flag is derived from
    /// `tab.tree.focused`, not from the caret. It stays true while the user is typing into a
    /// rename field, the commit message box or the Explorer's search input, so the binding would
    /// take Ctrl+F from every one of them in any window whose focused pane is a terminal.
    ///
    /// `terminal.find` is a registered command with a `terminalFocused` clause all the same, so
    /// it has a palette row and a user who wants the chord in the keymap — ⌘F on a Mac, say —
    /// writes one line of `keymap.json`. What must not happen is cide shipping that line.
    #[test]
    fn ctrl_f_is_not_bound_here_because_two_panes_mean_two_things_by_it() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            assert!(
                key != "ctrl+f" && key != "meta+f",
                "{key} is bound to {} — Ctrl+F is CodeMirror's find bar in an editor and the \
                 terminal's own focus-scoped chord in a pane, and the window capture gate would \
                 take it from both at once; see ui/src/terminal/keys.ts",
                binding.command
            );
        }
    }

    /// **No default may bind a bare printable key.** (M15)
    ///
    /// The sidebar trees' speed search is a *type-ahead*: pressing `t` in the explorer starts a
    /// search for `t`, exactly as it does in IDEA, Explorer and Finder. That works today because
    /// [`defaults`] happens to contain no unmodified single-character binding — every entry
    /// carries ctrl, alt, shift or meta, and the three bare exceptions are `f4`, `mouseback` and
    /// `mouseforward`, none of which is a character.
    ///
    /// It was a *coincidence* with nothing behind it, and the failure mode is severe and silent.
    /// `ui/src/keys/gate.ts` resolves global bindings on a **window capture** listener, which
    /// runs before the event reaches its target — so a default of, say, `r → git.pull` would eat
    /// the letter `r` in the file tree, in the rename box, in the commit message box, in
    /// CodeMirror and in every terminal. Speed search would go dead for that one letter with no
    /// error anywhere, and the tree would look like it had stopped listening.
    ///
    /// Written as an assertion about *what the default layer may contain* rather than about the
    /// tree, because the tree cannot defend itself: by the time its `onKeyDown` runs, the gate
    /// has already decided.
    #[test]
    fn no_default_binds_an_unmodified_printable_key() {
        for binding in defaults().into_iter().chain(platform_defaults()) {
            let key = normalize_key(&binding.key);
            // A modified chord is fine; so is a named key (`f4`, `escape`, `mouseback`), which
            // is more than one character and therefore not something a keyboard types into a
            // text field.
            if key.contains('+') || key.chars().count() != 1 {
                continue;
            }
            panic!(
                "`{key}` is bound to {} with no modifier. The key gate's window capture \
                 listener would swallow that character everywhere — the file tree's speed \
                 search, the rename box, the commit message, CodeMirror and every terminal — \
                 before the focused element ever saw it. Add a modifier, or the binding is a \
                 letter the user can no longer type.",
                binding.command
            );
        }
    }

    #[test]
    fn spellings_of_one_keystroke_all_normalise_alike() {
        let canonical = normalize_key("ctrl+shift+p");
        assert_eq!(normalize_key("Ctrl+Shift+P"), canonical);
        assert_eq!(normalize_key("shift+ctrl+p"), canonical);
        assert_eq!(normalize_key("SHIFT+CONTROL+P"), canonical);
        assert_eq!(canonical, "ctrl+shift+p");
    }

    #[test]
    fn modifiers_normalise_into_ctrl_alt_shift_meta_order() {
        assert_eq!(
            normalize_key("meta+shift+alt+ctrl+k"),
            "ctrl+alt+shift+meta+k"
        );
    }

    #[test]
    fn platform_modifier_aliases_normalise_to_meta() {
        assert_eq!(normalize_key("cmd+p"), "meta+p");
        assert_eq!(normalize_key("command+p"), "meta+p");
        assert_eq!(normalize_key("super+p"), "meta+p");
    }

    #[test]
    fn whitespace_between_strokes_collapses_to_one_space() {
        assert_eq!(normalize_key("ctrl+k \t ctrl+s"), "ctrl+k ctrl+s");
    }

    #[test]
    fn a_sequence_parses_into_one_chord_per_stroke() {
        let chords = parse_chord("ctrl+k ctrl+s").expect("valid sequence");
        assert_eq!(chords.len(), 2);
        assert!(chords[0].ctrl && chords[0].key == "k");
        assert!(chords[1].ctrl && chords[1].key == "s");
        assert!(!chords[0].alt && !chords[0].shift && !chords[0].meta);
    }

    #[test]
    fn a_literal_plus_is_a_bindable_key() {
        let chords = parse_chord("ctrl++").expect("plus is a key");
        assert_eq!(chords.len(), 1);
        assert!(chords[0].ctrl);
        assert_eq!(chords[0].key, "+");
        assert_eq!(normalize_key("Ctrl++"), "ctrl++");
    }

    #[test]
    fn a_leading_separator_is_an_error() {
        // `+p` must not quietly become `p`: that binds a bare letter the user never wrote,
        // which fires on ordinary typing everywhere the key is not consumed by a text field.
        assert!(parse_chord("+p").is_err());
        assert!(parse_chord("+ctrl+p").is_err());
        // The literal plus key keeps working, with and without modifiers.
        assert_eq!(normalize_key("+"), "+");
        assert_eq!(normalize_key("ctrl++"), "ctrl++");
    }

    #[test]
    fn an_unknown_modifier_is_an_error() {
        let err = parse_chord("hyper+p").expect_err("hyper is not a modifier");
        assert!(matches!(err, CoreError::Serde(_)), "{err:?}");
    }

    #[test]
    fn modifiers_without_a_key_are_an_error() {
        let err = parse_chord("ctrl+").expect_err("no key");
        assert!(matches!(err, CoreError::Serde(_)), "{err:?}");
    }

    #[test]
    fn an_empty_key_string_is_an_error() {
        assert!(parse_chord("").is_err());
        assert!(parse_chord("   ").is_err());
    }

    #[test]
    fn an_unparseable_key_normalises_instead_of_panicking() {
        assert_eq!(normalize_key("hyper+P"), "hyper+p");
    }

    #[test]
    fn the_default_layer_binds_every_command_the_palette_shows() {
        let resolved = resolve(&[]);
        for command in [
            "pane.split.right",
            "pane.split.down",
            "claude.split.newSession",
            // `pane.promoteToTab` was in this list and is not any more: the mock draws a
            // shortcut for it, but the command is `unavailable` and binding a key to an
            // unavailable command means the gate eats the keystroke and nothing happens.
            // The binding comes back with the domain operation.
            "pane.detachToWindow",
            "terminal.splitBelow",
            // The spawn-chord family — Ctrl+(/)/{/} in the ask's spelling, bound as the
            // physical keys Shift makes those faces of.
            "claude.split.right",
            "claude.addRow",
            "terminal.splitRight",
            "terminal.addRow",
            "picker.files",
            "palette.commands",
            "tab.close",
            "tab.reopenClosed",
            "file.save",
            // M15. `theme.toggle` was in this list and is deliberately gone: `ctrl+shift+t` is
            // now `tab.reopenClosed`, which is what that chord means everywhere else, and
            // switching the theme is a twice-a-year gesture that does not need a chord at all.
            // The command still exists, still appears in the palette and is still bindable in
            // `keymap.json`; it simply ships with no default. Listing it here would be
            // asserting a binding that does not exist — the same note as
            // `project.switcher.prev` below.
            "pane.navigate.left",
            "pane.navigate.right",
            "pane.navigate.up",
            "pane.navigate.down",
            // Alt+Enter, scoped `!editorFocused` — inside a buffer the chord is CodeMirror's
            // *send lines to Claude* and must stay so.
            "pane.maximize",
            "settings.open",
            "project.switcher.next",
            // M14. `project.switcher.prev` was in this list and is deliberately gone: its
            // reverse chord `ctrl+shift+backquote` is `terminal.splitBelow`, so it ships
            // palette-only and is reached by holding Shift during a walk. Listing it here
            // would be asserting a binding that does not exist.
            "tab.switcher.next",
            "tab.switcher.prev",
            "tab.console",
            "sidebar.toggle",
            "navigate.usages",
        ] {
            assert!(
                resolved.iter().any(|r| r.command == command),
                "{command} has no default binding"
            );
        }
    }

    #[test]
    fn every_default_key_parses() {
        // An unparseable default is invisible in every other test: `normalize_key` folds it
        // instead of failing, and `platform_layer` skips it, so it would ship as a key that
        // cannot be pressed and that macOS never gets a meta spelling for.
        for binding in defaults() {
            assert!(
                parse_chord(&binding.key).is_ok(),
                "default key `{}` does not parse",
                binding.key
            );
        }
    }

    #[test]
    fn every_default_command_is_registered() {
        // The registry's own test walks a hand-written list rather than this one, so a
        // default added here naming an id nobody registered would otherwise ship as a key
        // that silently does nothing.
        for binding in defaults() {
            assert!(
                crate::commands::by_id(binding.target_command()).is_some(),
                "default binds unregistered command `{}`",
                binding.command
            );
        }
    }

    /// Every clause on a **binding** names a flag the vocabulary knows. (M16)
    ///
    /// # This was a hole, and it was found by mutation
    ///
    /// `check-commands.mjs` has always asserted this for a `Command`'s `when` — the clause that
    /// gates the *palette* — and nothing asserted it for a `Binding`'s, which gates the
    /// *keyboard*. The split between those two is spelled out at the head of this module; what
    /// it did not have was a check on the second half.
    ///
    /// The failure is silent and total, and it is this project's signature defect wearing a
    /// typo. `KeyContext` is a map and evaluation reads a missing key as `false`, so a clause
    /// naming `pickerIsOpen` where the vocabulary says `filePickerOpen` is **permanently
    /// false**: the binding is in the table, `conflicts` reports nothing about it, Settings →
    /// Keymap lists it, and the chord does nothing for ever. Changing this file and
    /// `check-key-gate.mjs`'s mirror together — which is exactly what somebody renaming a flag
    /// would do — left the entire suite green.
    ///
    /// Here rather than in the TypeScript, because this side owns both tables and the failure
    /// should arrive where it was caused. `check-commands.mjs` keeps the other half, over a
    /// `CONTEXT_FLAGS` it reads out of this crate's source.
    ///
    /// Compound clauses are split into identifiers, so `a && !b` is checked as two names rather
    /// than as one string that matches nothing.
    #[test]
    fn every_default_binding_scopes_itself_with_a_flag_that_exists() {
        let mut checked = 0usize;
        // Both platform layers, because a clause added to the macOS-only pushes would never be
        // built on this machine and would ship without anybody having run it.
        let layers: Vec<Binding> = defaults()
            .into_iter()
            .chain(platform_layer(false))
            .chain(platform_layer(true))
            .collect();
        for binding in &layers {
            let Some(clause) = binding.when.as_deref() else {
                continue;
            };
            for flag in clause
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|part| !part.is_empty())
            {
                assert!(
                    crate::commands::CONTEXT_FLAGS.contains(&flag),
                    "the binding `{}` → `{}` is scoped to `{flag}`, which is not in \
                     CONTEXT_FLAGS. A key context is a map and a missing key reads as false, so \
                     this chord is dead in every context — listed in Settings → Keymap, \
                     reported by nothing, and doing nothing for ever.",
                    binding.key,
                    binding.command
                );
                checked += 1;
            }
        }
        assert!(
            checked >= 8,
            "only {checked} clause identifiers were checked — the scan stopped seeing them"
        );
    }

    #[test]
    fn the_default_keymap_has_no_conflicts() {
        assert_eq!(conflicts(&resolve(&[])), Vec::new());
    }

    #[test]
    fn the_alt_arrow_pane_moves_complement_the_member_walk() {
        let resolved = resolve(&[]);
        // The horizontal pair is unconditional; the vertical pair and Alt+Enter ship as the
        // `!editorFocused` complement of chords a buffer already owns. Both rows on a shared
        // key must survive resolution — a "tidied" merge that collapsed a key to one command
        // would take either the member walk or the pane move, silently.
        assert_eq!(command_for(&resolved, "alt+left"), ["pane.navigate.left"]);
        assert_eq!(command_for(&resolved, "alt+right"), ["pane.navigate.right"]);
        assert_eq!(
            command_for(&resolved, "alt+up"),
            ["navigate.prevMember", "pane.navigate.up"]
        );
        assert_eq!(
            command_for(&resolved, "alt+down"),
            ["navigate.nextMember", "pane.navigate.down"]
        );
        assert_eq!(command_for(&resolved, "alt+enter"), ["pane.maximize"]);
        // Disjoint by construction — one flag, negated — which is what keeps the shared keys
        // out of `conflicts` honestly rather than by mere string inequality.
        for binding in resolved
            .iter()
            .filter(|r| normalize_key(&r.key) == "alt+up")
        {
            match binding.command.as_str() {
                "navigate.prevMember" => {
                    assert_eq!(binding.when.as_deref(), Some("editorFocused"));
                }
                "pane.navigate.up" => {
                    assert_eq!(binding.when.as_deref(), Some("!editorFocused"));
                }
                other => panic!("unexpected command on alt+up: {other}"),
            }
        }
    }

    #[test]
    fn a_user_binding_shadows_the_default_on_the_same_key() {
        let user = vec![Binding::new("Shift+Ctrl+P", "picker.files")];
        let resolved = resolve(&user);
        assert_eq!(command_for(&resolved, "ctrl+shift+p"), ["picker.files"]);
    }

    #[test]
    fn shadowing_is_by_normalised_key_not_by_spelling() {
        // The default is written `ctrl+shift+p`; the user writes the modifiers the other way
        // round. Without normalisation both would survive and the override would appear dead.
        let resolved = resolve(&[Binding::new("SHIFT+CTRL+P", "palette.commands")]);
        let matching: Vec<_> = resolved
            .iter()
            .filter(|r| r.key == "ctrl+shift+p")
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].layer, KeymapLayer::User);
    }

    #[test]
    fn a_user_binding_with_a_different_when_coexists_with_the_default() {
        let user = vec![Binding::new("ctrl+p", "terminal.paste").when("terminalFocused")];
        let resolved = resolve_pc(&user);
        let mut commands = command_for(&resolved, "ctrl+p");
        commands.sort();
        assert_eq!(commands, ["picker.files", "terminal.paste"]);
    }

    #[test]
    fn a_removal_drops_the_default_it_names() {
        let resolved = resolve(&[Binding::new("ctrl+p", "-picker.files")]);
        assert!(command_for(&resolved, "ctrl+p").is_empty());
    }

    #[test]
    fn a_removal_leaves_other_commands_on_the_same_key_alone() {
        let user = vec![
            Binding::new("ctrl+p", "terminal.paste").when("terminalFocused"),
            Binding::new("ctrl+p", "-picker.files"),
        ];
        let resolved = resolve(&user);
        assert_eq!(command_for(&resolved, "ctrl+p"), ["terminal.paste"]);
    }

    #[test]
    fn a_removal_names_the_command_so_a_bare_dash_removes_nothing() {
        let before = resolve(&[]).len();
        let resolved = resolve(&[Binding::new("ctrl+p", "-")]);
        assert_eq!(resolved.len(), before);
    }

    #[test]
    fn a_removal_is_scoped_by_its_when_clause() {
        // `cide-ipc::keymap`'s own documented example. Matching on `when` as well as on
        // (key, command) is what makes it mean what it reads as, and is what VS Code — the
        // format this module implements — does.
        let user = vec![Binding::new("ctrl+w", "-tab.close").when("tabPinned")];
        let resolved = resolve_pc(&user);

        // The unconditional default is a different binding and survives untouched.
        assert_eq!(command_for(&resolved, "ctrl+w"), vec!["tab.close"]);
    }

    #[test]
    fn a_removal_matching_the_same_context_does_remove() {
        let user = vec![
            Binding::new("ctrl+w", "tab.close").when("tabPinned"),
            Binding::new("ctrl+w", "-tab.close").when("tabPinned"),
        ];
        let resolved = resolve_pc(&user);

        // Only the scoped one went; the unconditional default is still there.
        let pinned: Vec<_> = resolved
            .iter()
            .filter(|r| r.key == "ctrl+w" && r.when.as_deref() == Some("tabPinned"))
            .collect();
        assert!(pinned.is_empty(), "the scoped binding should be gone");
        assert_eq!(command_for(&resolved, "ctrl+w"), vec!["tab.close"]);
    }

    #[test]
    fn a_removal_that_matches_nothing_is_reported() {
        // The sharp edge of VS Code's removal semantics: forget the `when` and nothing
        // happens. Silence here is the exact failure this module exists to prevent.
        let user = vec![Binding::new("ctrl+w", "-tab.close").when("noSuchContext")];
        let (_, diags) = resolve_with_diagnostics(&user);

        assert!(
            diags.iter().any(|d| matches!(
                d,
                KeymapDiagnostic::RemovalMatchedNothing { command, .. } if command == "tab.close"
            )),
            "expected a RemovalMatchedNothing diagnostic, got {diags:?}"
        );
    }

    #[test]
    fn an_unparseable_key_is_reported_rather_than_silently_folded() {
        let user = vec![Binding::new("ctrl+", "file.save")];
        let (_, diags) = resolve_with_diagnostics(&user);

        assert!(
            diags.iter().any(
                |d| matches!(d, KeymapDiagnostic::UnparseableKey { key, .. } if key == "ctrl+")
            ),
            "expected an UnparseableKey diagnostic, got {diags:?}"
        );
    }

    #[test]
    fn every_compiled_in_binding_parses() {
        // A diagnostic from the default or platform layer would mean a typo shipped in this
        // crate, not a mistake in a user's file.
        let (_, diags) = resolve_with_diagnostics(&[]);
        assert!(
            diags.is_empty(),
            "compiled-in bindings are not clean: {diags:?}"
        );
    }

    #[test]
    fn a_when_clause_differing_only_by_whitespace_still_overrides() {
        // Keys are normalised, so contexts must be too — otherwise an override fails for a
        // reason invisible in the file.
        let user = vec![Binding::new("ctrl+p", "picker.symbols").when("  editorFocused  ")];
        let base = vec![Binding::new("ctrl+p", "picker.files").when("editorFocused")];
        let mut out = Vec::new();
        let mut diags = Vec::new();
        apply_layer(&mut out, KeymapLayer::Default, &base, &mut diags);
        apply_layer(&mut out, KeymapLayer::User, &user, &mut diags);

        let bound: Vec<_> = out.iter().filter(|r| r.key == "ctrl+p").collect();
        assert_eq!(bound.len(), 1, "the override should replace, not coexist");
        assert_eq!(bound[0].command, "picker.symbols");
    }

    #[test]
    fn resolved_bindings_carry_the_layer_they_came_from() {
        let resolved = resolve_pc(&[Binding::new("ctrl+s", "file.saveAll")]);
        let overridden = resolved
            .iter()
            .find(|r| r.key == "ctrl+s")
            .expect("ctrl+s stays bound");
        assert_eq!(overridden.layer, KeymapLayer::User);
        let untouched = resolved
            .iter()
            .find(|r| r.command == "tab.close")
            .expect("tab.close stays bound");
        assert_eq!(untouched.layer, KeymapLayer::Default);
    }

    #[test]
    fn the_macos_layer_rebinds_ctrl_defaults_onto_meta() {
        let resolved = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(command_for(&resolved, "meta+p"), ["picker.files"]);
        assert!(
            command_for(&resolved, "ctrl+p").is_empty(),
            "the ctrl spelling must not survive on macOS"
        );
        let mac_binding = resolved
            .iter()
            .find(|r| r.command == "picker.files")
            .expect("picker.files stays bound");
        assert_eq!(mac_binding.layer, KeymapLayer::Platform);
    }

    #[test]
    fn the_macos_layer_leaves_ctrl_tab_and_ctrl_backquote_alone() {
        // Both keys are eaten by macOS in their ⌘ spelling — `⌘⇥` is the application switcher
        // and `⌘\`` / `⇧⌘\`` walk an application's windows — so rewriting either would
        // silently *remove* the switcher on that platform rather than move it.
        let resolved = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(command_for(&resolved, "ctrl+tab"), ["tab.switcher.next"]);
        assert_eq!(
            command_for(&resolved, "ctrl+shift+tab"),
            ["tab.switcher.prev"]
        );
        assert!(
            command_for(&resolved, "meta+tab").is_empty(),
            "nothing may be bound to the macOS application switcher"
        );

        assert_eq!(
            command_for(&resolved, "ctrl+`"),
            ["project.switcher.next"],
            "⌘` is macOS's own next-window shortcut, so the project switcher keeps ⌃"
        );
        assert_eq!(
            command_for(&resolved, "ctrl+shift+`"),
            ["terminal.splitBelow"],
            "and its neighbour on the same physical key keeps ⌃ for the same reason"
        );
        for eaten in ["meta+`", "shift+meta+`"] {
            assert!(
                command_for(&resolved, eaten).is_empty(),
                "{eaten} is a macOS window shortcut and must stay unbound"
            );
        }

        // And the exception is narrow: everything else still moves to ⌘.
        assert_eq!(command_for(&resolved, "meta+w"), ["tab.close"]);
        assert_eq!(command_for(&resolved, "meta+1"), ["tab.console"]);
    }

    /// F4 is the panel toggle, and the one-line escape hatch the README prints actually works.
    ///
    /// The binding costs every terminal pane in the shell window the `ESC O S` xterm sends for
    /// F4 — a real key to `mc`, `htop` and any curses TUI — and the whole justification for
    /// making that trade is that a user can take it back. An escape hatch nobody exercises is a
    /// claim, so this exercises it, including the `when`, which is the half a user will leave
    /// out: removal matches on (key, command, `when`), so the unscoped line reads correctly and
    /// does nothing.
    ///
    /// `f3` is asserted beside it because the two are one key apart and `f3` is find-next inside
    /// the editor — a resolver that folded f-keys together would take the find bar's last chord.
    #[test]
    fn f4_toggles_the_panel_and_can_be_given_back_to_the_terminal() {
        let shipped = resolve(&[]);
        assert_eq!(command_for(&shipped, "f4"), ["sidebar.toggle"]);
        assert!(command_for(&shipped, "f3").is_empty());

        let scoped = resolve(&[Binding::new("f4", "-sidebar.toggle").when("shellWindow")]);
        assert!(
            command_for(&scoped, "f4").is_empty(),
            "the documented one-liner has to actually give ESC O S back to the pty"
        );

        let unscoped = resolve(&[Binding::new("f4", "-sidebar.toggle")]);
        assert_eq!(
            command_for(&unscoped, "f4"),
            ["sidebar.toggle"],
            "a removal that forgets the `when` matches nothing — the README must not print it"
        );
    }

    /// Ctrl+Tab is the tab switcher; the project switcher moved to Ctrl+` and took no migration.
    ///
    /// The interesting half is what happens to a `keymap.json` that already named the old
    /// default, because that file is the reason ids are never renamed. Two lines, two outcomes,
    /// and only one of them is a surprise:
    ///
    /// * a **rebind** — `ctrl+tab` naming `project.switcher.next` — overrides on (key, `when`),
    ///   so the user keeps exactly the behaviour they configured and the tab switcher is simply
    ///   unbound for them. No conflict, no diagnostic, nothing to migrate.
    /// * an **unbind** — the same key with a leading `-` — matches on (key, command, `when`) and
    ///   now matches nothing, so it is reported as `RemovalMatchedNothing` *and* Ctrl+Tab starts
    ///   running a command they never asked for. That is the one behaviour change worth a
    ///   release note, and it is asserted rather than described.
    #[test]
    fn ctrl_tab_switches_tabs_and_an_old_keymap_json_still_wins() {
        let shipped = resolve_pc(&[]);
        assert_eq!(command_for(&shipped, "ctrl+tab"), ["tab.switcher.next"]);
        assert_eq!(command_for(&shipped, "ctrl+`"), ["project.switcher.next"]);
        assert_eq!(command_for(&shipped, "ctrl+1"), ["tab.console"]);
        assert!(
            command_for(&shipped, "ctrl+shift+`").contains(&"terminal.splitBelow".to_owned()),
            "the reverse project chord is taken, which is why `.prev` ships unbound"
        );

        // The old default's `when` is `shellWindow`, and a user line with no clause is a
        // *different* binding — so both survive and the user's, being last, wins the resolve.
        // Asserted as the whole list rather than as "theirs is in there", because a shadowed
        // default that still fires in some other context would be the silent half of this.
        let rebound = resolve_pc(&[Binding::new("ctrl+tab", "project.switcher.next")]);
        let mut on_tab = command_for(&rebound, "ctrl+tab");
        on_tab.sort();
        assert_eq!(on_tab, ["project.switcher.next", "tab.switcher.next"]);
        // ...and that IS a conflict, which this test used to assert it was not.
        //
        // The old assertion read "different `when`, no conflict", which is true of two *scoped*
        // bindings and false here: the user's line carries no clause, so it applies in every
        // context including `shellWindow`, and in a shell window Ctrl+Tab now has two applicable
        // commands with theirs winning on layer order. A keymap editor whose whole purpose is to
        // surface contested keys reported nothing about the one key this release moved — the
        // user would have found the new tab switcher simply absent, with no row explaining why.
        //
        // Reported against the scoped group, because naming the context is what makes the report
        // actionable: it says *where* the two overlap.
        assert_eq!(
            conflicts(&rebound),
            vec![Conflict {
                key: "ctrl+tab".to_owned(),
                commands: vec![
                    "tab.switcher.next".to_owned(),
                    "project.switcher.next".to_owned()
                ],
                when: Some("shellWindow".to_owned()),
            }],
            "an unscoped user binding contests every scoped binding on the same key, and this is \
             the release that made that reachable"
        );

        let unbound = resolve_pc(&[Binding::new("ctrl+tab", "-project.switcher.next")]);
        assert_eq!(
            command_for(&unbound, "ctrl+tab"),
            ["tab.switcher.next"],
            "an unbind of the old command no longer matches anything"
        );
        let (_, diags) =
            resolve_with_diagnostics_pc(&[Binding::new("ctrl+tab", "-project.switcher.next")]);
        assert!(
            diags.iter().any(|d| matches!(
                d,
                KeymapDiagnostic::RemovalMatchedNothing { command, .. }
                    if command == "project.switcher.next"
            )),
            "and the user is told, in Settings → Keymap, rather than left guessing: {diags:?}"
        );
    }

    #[test]
    fn the_macos_layer_keeps_the_other_modifiers_of_a_default() {
        let resolved = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(command_for(&resolved, "alt+meta+h"), ["pane.navigate.left"]);
    }

    /// Find in files lands on ⇧⌘F, which is what a Mac user's fingers already do.
    ///
    /// Pinned rather than left to the blanket rewrite, because this is one of the few defaults
    /// whose macOS spelling is a *claim* and not just a consequence: ⇧⌘F is find-in-files in
    /// Xcode, VS Code and every editor on the platform, and the comment beside the binding says
    /// so as the reason it needs no entry in [`keeps_ctrl_on_macos`]. If the rewrite ever grows
    /// an exception that catches `f`, the chord silently becomes ⌃⇧F on a platform where ⌃ is
    /// text navigation, and nothing else here would notice.
    #[test]
    fn find_in_files_is_the_platform_chord_on_macos_too() {
        let resolved = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(
            command_for(&resolved, "shift+meta+f"),
            ["sidebar.search"],
            "⇧⌘F is find-in-files on macOS"
        );
        assert!(
            command_for(&resolved, "ctrl+shift+f").is_empty(),
            "the ctrl spelling must not survive on macOS, where ⌃ is text navigation"
        );
        // And on everything else it stays where it was written.
        let linux = resolve_layers(&platform_layer(false), &[]);
        assert_eq!(command_for(&linux, "ctrl+shift+f"), ["sidebar.search"]);
    }

    /// Ctrl+T pulls, and the escape hatch that pays for it actually works.
    ///
    /// The binding is a deliberate trade — it shadows readline's `transpose-chars`, fzf's
    /// file widget and vim's tag-jump in every pane — and the whole justification for making
    /// it is that one line of `~/.config/cide/keymap.json` takes it back. An escape hatch
    /// nobody exercises is a claim, so this exercises it: with the removal in place the chord
    /// resolves to nothing, which is what makes the gate pass the keystroke through to the
    /// pty (`ui/src/keys/gate.ts` answers `passThrough` for an unbound chord).
    ///
    /// `ctrl+shift+t` is asserted beside it because the two are one keystroke apart and a
    /// resolver that folded shift away would reopen a closed tab on every pull. That used to
    /// read "silently swap the theme", which was the same claim about the same defect — the
    /// chord's command changed in M15 and the reason for asserting it did not.
    #[test]
    fn ctrl_t_pulls_and_can_be_given_back_to_the_terminal() {
        let shipped = resolve_pc(&[]);
        assert_eq!(command_for(&shipped, "ctrl+t"), ["git.pull"]);
        assert_eq!(command_for(&shipped, "ctrl+shift+t"), ["tab.reopenClosed"]);

        let freed = resolve_pc(&[Binding::new("ctrl+t", "-git.pull")]);
        assert!(
            command_for(&freed, "ctrl+t").is_empty(),
            "the documented one-liner has to actually restore transpose-chars"
        );
        assert_eq!(
            command_for(&freed, "ctrl+shift+t"),
            ["tab.reopenClosed"],
            "and it must not take the neighbouring chord with it"
        );
    }

    /// A registered command with no default binding is a supported state, and `theme.toggle`
    /// is now the second one in this table.
    ///
    /// Worth a test of its own rather than an absence, because "every palette row has a key" is
    /// the assertion a future reader is most likely to *add* — it sounds like an invariant and
    /// it is not one. Two commands deliberately ship unbound (`project.switcher.prev`, whose
    /// reverse chord is taken, and this one, whose chord was worth more to another command), and
    /// both are still reachable: the palette lists them, and `keymap.json` binds them.
    #[test]
    fn theme_toggle_ships_unbound_and_is_still_a_command() {
        let shipped = resolve(&[]);
        assert!(
            !shipped.iter().any(|r| r.command == "theme.toggle"),
            "no default layer binds it any more"
        );
        assert!(
            crate::commands::by_id("theme.toggle").is_some(),
            "and it is still in the registry, so the palette still offers it"
        );

        // And a user can take it back with one line, which is the whole of what it lost.
        let bound = resolve(&[Binding::new("alt+shift+d", "theme.toggle")]);
        assert_eq!(command_for(&bound, "alt+shift+d"), ["theme.toggle"]);
    }

    /// Ctrl+G goes to a line, and the escape hatch the README offers for it actually works —
    /// **including the `when`, which is the half a user will leave out.**
    ///
    /// The binding costs something real: `@codemirror/search` binds `Mod-g` to find-next, and the
    /// gate resolves before CodeMirror is offered the event, so inside an editor that stops
    /// working. The justification for making the trade anyway is that one line of
    /// `~/.config/cide/keymap.json` takes it back, and an escape hatch nobody exercises is a
    /// claim rather than a feature — the same reasoning as
    /// [`ctrl_t_pulls_and_can_be_given_back_to_the_terminal`] above.
    ///
    /// The sharp edge is `when`. Removal matches on (key, command, `when`), so the unscoped
    /// removal a user would write first matches *nothing* and the chord stays bound — with a
    /// `RemovalMatchedNothing` diagnostic and no visible change. Both halves are asserted here so
    /// the README cannot document the wrong line.
    #[test]
    fn ctrl_g_goes_to_a_line_and_can_be_given_back_to_codemirror() {
        let shipped = resolve_pc(&[]);
        assert_eq!(command_for(&shipped, "ctrl+g"), ["navigate.line"]);
        // The neighbouring chord is a *different* stroke, so find-previous survives untouched —
        // which is the asymmetry the binding's own comment names.
        assert!(command_for(&shipped, "ctrl+shift+g").is_empty());

        let scoped = resolve_pc(&[Binding::new("ctrl+g", "-navigate.line").when("editorFocused")]);
        assert!(
            command_for(&scoped, "ctrl+g").is_empty(),
            "the documented one-liner has to actually give Mod-g back to the editor"
        );

        let unscoped = resolve_pc(&[Binding::new("ctrl+g", "-navigate.line")]);
        assert_eq!(
            command_for(&unscoped, "ctrl+g"),
            ["navigate.line"],
            "a removal that forgets the `when` matches nothing — the README must not print it"
        );
        let (_, diags) = resolve_with_diagnostics_pc(&[Binding::new("ctrl+g", "-navigate.line")]);
        assert!(
            diags.iter().any(|d| matches!(
                d,
                KeymapDiagnostic::RemovalMatchedNothing { command, .. } if command == "navigate.line"
            )),
            "and the user is told, rather than left with a line that looks right: {diags:?}"
        );
    }

    /// Find usages answers ⌥F7, and only where there is a caret. (M14)
    ///
    /// The clause is the whole test. F7 is a real byte to a terminal application — `mc` puts its
    /// menu on the F keys — and the gate is a window *capture* listener, so an unscoped binding
    /// would swallow ⌥F7 in every terminal pane in every window for a command that needs a caret
    /// to mean anything. The escape hatch is asserted here for the same reason ⌃G's is: so the
    /// README cannot print a line that matches nothing.
    #[test]
    fn alt_f7_finds_usages_and_only_where_a_caret_is() {
        let shipped = resolve(&[]);
        assert_eq!(command_for(&shipped, "alt+f7"), ["navigate.usages"]);
        assert!(
            shipped
                .iter()
                .any(|r| r.command == "navigate.usages"
                    && r.when.as_deref() == Some("editorFocused")),
            "unscoped, this takes ⌥F7 from every terminal in the app"
        );
        // Nothing else in the table wants an F-key with **Alt**, so nothing is being displaced.
        // Bare F7 is the changes iterator since M25 (`navigate.nextChange`) and that is not a
        // displacement: a modifier is part of the stroke, so the two resolve independently and
        // ⌥F7 goes on meaning Find usages wherever there is a caret.
        assert_eq!(command_for(&shipped, "f7"), ["navigate.nextChange"]);

        let scoped = resolve(&[Binding::new("alt+f7", "-navigate.usages").when("editorFocused")]);
        assert!(
            command_for(&scoped, "alt+f7").is_empty(),
            "the documented one-liner has to actually give ⌥F7 back"
        );
        let unscoped = resolve(&[Binding::new("alt+f7", "-navigate.usages")]);
        assert_eq!(
            command_for(&unscoped, "alt+f7"),
            ["navigate.usages"],
            "a removal that forgets the `when` matches nothing — the README must not print it"
        );
    }

    #[test]
    fn the_thumb_buttons_are_bound_here_like_any_other_key() {
        let shipped = resolve(&[]);
        assert_eq!(command_for(&shipped, "mouseback"), ["navigate.back"]);
        assert_eq!(command_for(&shipped, "mouseforward"), ["navigate.forward"]);

        // Unconditional, unlike every other M12 binding. A thumb button is pressed wherever the
        // pointer is — in this app, most often over a terminal — and unlike ⌃B or ⌃G there is
        // nothing to take away: no shell reads GDK button 8.
        for binding in defaults() {
            if binding.key.contains("mouse") {
                assert_eq!(
                    binding.when, None,
                    "{} must stay unscoped, or Back does nothing wherever the pointer usually is",
                    binding.key
                );
            }
        }

        // The whole point of routing the mouse through the keymap: a user can rebind or unbind
        // it with one line, exactly as for a chord. No `when`, so no `when` on the removal
        // either — the trap `ctrl_g_goes_to_a_line_and_can_be_given_back_to_codemirror` pins.
        let unbound = resolve(&[Binding::new("mouseback", "-navigate.back")]);
        assert!(command_for(&unbound, "mouseback").is_empty());
        let rebound = resolve(&[Binding::new("mouseback", "navigate.definition")]);
        assert_eq!(
            command_for(&rebound, "mouseback"),
            ["navigate.definition"],
            "a rebind shadows the default rather than failing to parse"
        );

        // `parse_stroke` validates modifiers only, so a token that is not a key name needs no
        // parser change — and `ctrl+mouseback` is a legal thing for a user to write.
        let chord = parse_chord("ctrl+mouseback").expect("a modified thumb press parses");
        assert_eq!(chord.len(), 1);
        assert_eq!(chord[0].key, "mouseback");
        assert!(chord[0].ctrl);
        assert_eq!(normalize_key("Ctrl+MouseBack"), "ctrl+mouseback");
    }

    #[test]
    fn the_macos_layer_leaves_the_thumb_buttons_alone() {
        // They carry no `ctrl`, so `ctrl_to_meta` has nothing to rewrite — asserted rather than
        // assumed, because a blanket rewrite that produced `meta+mouseback` would leave macOS
        // with two bindings for one button and no way to tell which fired.
        let mac = platform_layer(true);
        assert!(
            !mac.iter().any(|b| b.key.contains("mouse")),
            "the macOS layer must not touch a button that has no ctrl to rewrite: {mac:?}"
        );
        let resolved = resolve_layers(&mac, &[]);
        assert_eq!(command_for(&resolved, "mouseback"), ["navigate.back"]);
    }

    /// The chords a Mac user will press and nothing will happen, pinned in both directions.
    ///
    /// Not a lint and not a wish: it is an allowlist of conflicts somebody looked at and decided
    /// to live with, and it fails when the set **grows** (a new binding is dead on macOS and
    /// nobody noticed) *and* when it **shrinks** (an entry here has gone stale and is now
    /// misleading prose in `README.md`). Set equality is the only assertion that catches both.
    ///
    /// It cannot be a behavioural test, because the thing that eats the keystroke is AppKit
    /// resolving a menu accelerator in a process this repository has never run.
    #[test]
    fn every_macos_menu_conflict_is_one_somebody_decided_to_keep() {
        let acknowledged = [
            // ⌘W. The Mac-correct binding for closing a *tab* — it is what Safari, Chrome and
            // VS Code do — so the binding is right and the obstacle is the menu, not the
            // keymap. Kept rather than moved back to ctrl+w, which would be wrong on the
            // platform's own terms; the fix is `.enable_macos_default_menu(false)` plus an
            // explicit menu whose accelerators match `commands.rs`, and that is a decision
            // that needs somebody looking at a Mac. Until then ⌘W closes the window.
            ("meta+w", "tab.close", "File → Close Window"),
            // ⌥⌘H. Hide Others, which is a strong system convention and not worth fighting.
            // The other three of the family (⌥⌘J/K/L) are unclaimed, so pane navigation works
            // in three directions out of four there — which is worse than it sounds, and is
            // the reason this is written down rather than shrugged at.
            ("alt+meta+h", "pane.navigate.left", "cide → Hide Others"),
        ];

        let found: Vec<(String, String, &str)> = macos_menu_conflicts()
            .into_iter()
            .map(|c| (c.key, c.command, c.menu_item))
            .collect();
        let expected: Vec<(String, String, &str)> = acknowledged
            .iter()
            .map(|(k, c, m)| ((*k).to_string(), (*c).to_string(), *m))
            .collect();

        assert_eq!(
            found, expected,
            "the set of default bindings macOS's menu bar makes unreachable has changed. A new \
             entry means a chord that is listed in Settings → Keymap, resolves cleanly, and can \
             never fire on a Mac — AppKit answers the accelerator before WKWebView is asked. \
             Either move the binding, or add it here with the reason it is acceptable, and keep \
             README.md's Platforms section in step"
        );
    }

    /// A chord the layer *removes* is not a chord that stopped working.
    ///
    /// Reachable, not hypothetical: a default written `ctrl+meta+f` produces an unbind on that
    /// exact key, and ⌃⌘F is Full Screen, so without the filter the list above would carry a
    /// permanent entry describing a binding that no longer exists. There is no such default
    /// today, which is precisely why this is a direct test rather than a mutation of `defaults`.
    #[test]
    fn an_unbind_on_a_claimed_chord_is_not_reported_as_a_dead_binding() {
        let removed = Binding {
            key: "ctrl+meta+f".into(),
            command: "-view.fullscreen".into(),
            when: None,
            args: None,
        };
        let kept = Binding::new("ctrl+meta+f", "view.fullscreen");

        assert!(conflicts_in(vec![removed.clone()]).is_empty());
        assert_eq!(
            conflicts_in(vec![removed, kept])
                .into_iter()
                .map(|c| c.command)
                .collect::<Vec<_>>(),
            ["view.fullscreen"],
            "the rebind on a claimed chord is still a conflict; only the removal is not"
        );
    }

    /// The claimed-chord table is only as good as its spelling.
    ///
    /// Every entry has to parse and re-render to itself, or it can never match a resolved
    /// binding and the whole check above quietly finds nothing — the vacuous-green failure this
    /// project has paid for more than once.
    #[test]
    fn every_chord_the_mac_menu_bar_claims_is_spelled_the_way_this_module_spells_one() {
        for (chord, item) in MACOS_MENU_CHORDS {
            let parsed = parse_chord(chord).unwrap_or_else(|e| panic!("{item}: {chord}: {e:?}"));
            assert_eq!(
                render(&parsed),
                chord,
                "{item}: `{chord}` is not this module's canonical spelling (modifiers go \
                 ctrl, alt, shift, meta), so it can never match a resolved binding"
            );
        }
    }

    #[test]
    fn macos_gets_cmd_bracket_for_back_and_forward() {
        let mac = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(command_for(&mac, "meta+bracketleft"), ["navigate.back"]);
        assert_eq!(
            command_for(&mac, "meta+bracketright"),
            ["navigate.forward"],
            "⌘] is Forward on macOS — the chord Xcode, VS Code and every browser use"
        );

        // The literal spelling has to resolve to the same stroke, because that is what a user
        // writes in `keymap.json` and what `chords.ts` folds an event onto.
        assert_eq!(normalize_key("Meta+["), "meta+[");
        assert_eq!(normalize_key("meta+bracketleft"), "meta+bracketleft");

        // Linux and Windows get nothing: `ctrl+[` IS the ESC byte and `ctrl+]` is telnet's
        // escape, and the gate is a window capture listener, so binding either would take them
        // from every terminal pane in every window.
        let linux = resolve_layers(&platform_layer(false), &[]);
        for key in ["meta+bracketleft", "meta+bracketright", "ctrl+[", "ctrl+]"] {
            assert!(
                command_for(&linux, key).is_empty(),
                "{key} must be unbound off macOS"
            );
        }
        // *Unshifted*, precisely. This assertion used to forbid every bracket chord, and its
        // stated reason was always the narrower rule: the byte theft is `ctrl+[`'s, and xterm
        // encodes a control byte only for `ctrl && !shift && !alt && !meta` — the same fact
        // that leaves ⌃⇧C inert. The spawn chords (`ctrl+shift+bracketleft`/`right`, asked
        // for as CTRL+{ / CTRL+}) stand on exactly that distinction, so the guard now states
        // the rule it was protecting rather than a superset of it.
        for binding in defaults() {
            for chord in parse_chord(&binding.key).expect("every default parses") {
                let bracket = matches!(
                    chord.key.as_str(),
                    "[" | "]" | "bracketleft" | "bracketright"
                );
                assert!(
                    !bracket || chord.shift,
                    "{} is bound to {} — an unshifted bracket chord in `defaults()` reaches \
                     Linux, where ctrl+[ is the ESC character every terminal application reads",
                    binding.key,
                    binding.command
                );
            }
        }

        // Rebindable and unbindable like any other key, which is the whole reason these live in
        // the keymap rather than in a mac-only branch of the gate.
        let unbound = resolve_layers(
            &platform_layer(true),
            &[Binding::new("meta+bracketleft", "-navigate.back")],
        );
        assert!(command_for(&unbound, "meta+bracketleft").is_empty());
    }

    #[test]
    fn the_non_macos_platform_layer_changes_nothing() {
        assert!(platform_layer(false).is_empty());
        let resolved = resolve_layers(&platform_layer(false), &[]);
        assert_eq!(resolved.len(), defaults().len());
        assert!(resolved.iter().all(|r| r.layer == KeymapLayer::Default));
    }

    #[test]
    fn a_user_binding_still_wins_over_the_macos_layer() {
        let user = vec![Binding::new("meta+p", "palette.commands")];
        let resolved = resolve_layers(&platform_layer(true), &user);
        assert_eq!(command_for(&resolved, "meta+p"), ["palette.commands"]);
    }

    #[test]
    fn two_commands_on_one_key_in_the_same_context_conflict() {
        let user = vec![
            Binding::new("ctrl+alt+p", "picker.files"),
            Binding::new("Alt+Ctrl+P", "palette.commands"),
        ];
        let found = conflicts(&resolve(&user));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "ctrl+alt+p");
        assert_eq!(found[0].commands, ["picker.files", "palette.commands"]);
        assert_eq!(found[0].when, None);
    }

    #[test]
    fn the_same_key_with_different_when_clauses_is_not_a_conflict() {
        let user = vec![
            Binding::new("ctrl+alt+p", "picker.files").when("editorFocused"),
            Binding::new("ctrl+alt+p", "terminal.paste").when("terminalFocused"),
        ];
        assert_eq!(conflicts(&resolve(&user)), Vec::new());
    }

    #[test]
    fn one_command_bound_twice_on_a_key_is_not_a_conflict() {
        let user = vec![
            Binding::new("ctrl+alt+p", "picker.files"),
            Binding::new("ctrl+alt+p", "picker.files"),
        ];
        assert_eq!(conflicts(&resolve(&user)), Vec::new());
    }

    // --- editing the user layer ---------------------------------------------------------
    //
    // The traps these cover are the ones already documented a few hundred lines up: a removal
    // matches on (key, command, `when`) and does nothing at all when any of the three is
    // wrong; duplicates within a layer coexist rather than shadowing; and the file is a diff
    // against compiled-in defaults, so anything an editor writes that is not a change is a
    // default frozen into a user's file.

    /// The one-line summary of a `keymap.json`, for asserting on its *contents* rather than
    /// on what it resolves to. `key command when` per entry, in file order.
    fn spell(user: &[Binding]) -> Vec<String> {
        user.iter()
            .map(|b| match &b.when {
                Some(when) => format!("{} {} when {when}", b.key, b.command),
                None => format!("{} {}", b.key, b.command),
            })
            .collect()
    }

    #[test]
    fn rebinding_a_default_writes_a_removal_and_an_add_and_nothing_else() {
        let mut user = Vec::new();
        let outcome = edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "picker.files".into(),
                when: None,
                key: "ctrl+shift+o".into(),
            },
        );

        // Two entries, and the removal first: the add is on a different key, so the order does
        // not decide the outcome — but a file a user reads should say "take this away, put it
        // there" in that order.
        assert_eq!(
            spell(&user),
            ["ctrl+p -picker.files", "ctrl+shift+o picker.files"]
        );
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 0,
                added: 2
            }
        );

        let resolved = resolve_pc(&user);
        assert_eq!(command_for(&resolved, "ctrl+shift+o"), ["picker.files"]);
        assert!(
            command_for(&resolved, "ctrl+p").is_empty(),
            "leaving the default in place is how one command ends up bound twice"
        );
        assert_eq!(conflicts(&resolved), Vec::new());
    }

    #[test]
    fn rebinding_copies_the_when_of_the_binding_it_moves() {
        // `tab.console` carries `shellWindow`, and the clause is the whole of its manners: a
        // detached-pane window has no tab strip, and an unscoped chord would be swallowed
        // there for a diagnostic log line. An editor that dropped the clause would silently
        // widen every scoped binding it touched.
        let mut user = Vec::new();
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "tab.console".into(),
                when: Some("shellWindow".into()),
                key: "ctrl+0".into(),
            },
        );

        assert_eq!(
            spell(&user),
            [
                "ctrl+1 -tab.console when shellWindow",
                "ctrl+0 tab.console when shellWindow",
            ]
        );

        let resolved = resolve_pc(&user);
        let moved = resolved
            .iter()
            .find(|r| r.command == "tab.console")
            .expect("still bound");
        assert_eq!(moved.key, "ctrl+0");
        assert_eq!(moved.when.as_deref(), Some("shellWindow"));
        assert!(command_for(&resolved, "ctrl+1").is_empty());
    }

    #[test]
    fn a_second_rebind_replaces_the_first_instead_of_stacking_on_it() {
        // The sharp edge of "duplicates within a layer coexist": appending without stripping
        // leaves the previous override alive, and `conflicts` then reports a fight the editor
        // itself started. Three rebinds, two entries.
        let mut user = Vec::new();
        for key in ["ctrl+shift+o", "alt+o", "ctrl+alt+o"] {
            edit_pc(
                &mut user,
                &KeymapEdit::Rebind {
                    command: "picker.files".into(),
                    when: None,
                    key: key.into(),
                },
            );
        }

        assert_eq!(
            spell(&user),
            ["ctrl+p -picker.files", "ctrl+alt+o picker.files"]
        );
        let resolved = resolve_pc(&user);
        assert_eq!(command_for(&resolved, "ctrl+alt+o"), ["picker.files"]);
        assert_eq!(conflicts(&resolved), Vec::new());
    }

    #[test]
    fn rebinding_back_onto_the_default_chord_leaves_no_override_at_all() {
        // Not "write the default in again". The default is compiled in and reappears the
        // moment nothing in the user layer stands on it, and an entry saying `ctrl+p
        // picker.files` would freeze *today's* default into the file — the exact failure the
        // overrides-only format exists to avoid.
        let mut user = Vec::new();
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "picker.files".into(),
                when: None,
                key: "ctrl+shift+o".into(),
            },
        );
        let outcome = edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "picker.files".into(),
                when: None,
                key: "Ctrl+P".into(),
            },
        );

        assert_eq!(user, Vec::new(), "a rebind back to the default is a reset");
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 2,
                added: 0
            }
        );
        assert_eq!(command_for(&resolve_pc(&user), "ctrl+p"), ["picker.files"]);
    }

    #[test]
    fn unbinding_writes_exactly_the_one_liner_the_readme_prints() {
        // F4 costs every terminal pane in the shell window the `ESC O S` xterm sends for it,
        // and the whole justification for that trade is a line of `keymap.json` that takes it
        // back — including the `when`, which is the half a user leaves out. The screen has to
        // produce that line and not the one that reads correctly and does nothing.
        let mut user = Vec::new();
        let outcome = apply_edit(
            &mut user,
            &KeymapEdit::Unbind {
                command: "sidebar.toggle".into(),
                when: Some("shellWindow".into()),
            },
        );

        assert_eq!(spell(&user), ["f4 -sidebar.toggle when shellWindow"]);
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 0,
                added: 1
            }
        );
        assert!(
            command_for(&resolve(&user), "f4").is_empty(),
            "F4 has to actually reach the pty again"
        );
        let (_, diags) = resolve_with_diagnostics(&user);
        assert!(
            diags.is_empty(),
            "and the removal has to match something: {diags:?}"
        );
    }

    #[test]
    fn unbinding_a_command_the_user_had_already_moved_leaves_one_removal() {
        let mut user = Vec::new();
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "git.pull".into(),
                when: None,
                key: "ctrl+alt+t".into(),
            },
        );
        edit_pc(
            &mut user,
            &KeymapEdit::Unbind {
                command: "git.pull".into(),
                when: None,
            },
        );

        // The add is gone and the default is still suppressed — not two removals, and not a
        // removal aimed at the key the user had chosen, which would match nothing.
        assert_eq!(spell(&user), ["ctrl+t -git.pull"]);
        let resolved = resolve_pc(&user);
        assert!(command_for(&resolved, "ctrl+t").is_empty());
        assert!(command_for(&resolved, "ctrl+alt+t").is_empty());
    }

    #[test]
    fn reset_deletes_the_pair_and_the_default_comes_back() {
        let mut user = vec![Binding::new("ctrl+shift+u", "some.other.command")];
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "file.save".into(),
                when: None,
                key: "ctrl+alt+s".into(),
            },
        );
        assert_eq!(user.len(), 3);

        let outcome = edit_pc(
            &mut user,
            &KeymapEdit::Reset {
                command: "file.save".into(),
                when: None,
            },
        );

        assert_eq!(
            outcome,
            EditOutcome {
                removed: 2,
                added: 0
            }
        );
        assert_eq!(
            spell(&user),
            ["ctrl+shift+u some.other.command"],
            "a reset touches this command's entries and nobody else's"
        );
        assert_eq!(command_for(&resolve_pc(&user), "ctrl+s"), ["file.save"]);
    }

    #[test]
    fn a_reset_that_matches_nothing_says_so() {
        // The `when` trap arriving through the reset door. `file.save` is unconditional, so a
        // reset scoped to a context matches none of its entries — and a screen that reported
        // success here would be describing a file it had not changed.
        let mut user = Vec::new();
        apply_edit(
            &mut user,
            &KeymapEdit::Rebind {
                command: "file.save".into(),
                when: None,
                key: "ctrl+alt+s".into(),
            },
        );
        let outcome = apply_edit(
            &mut user,
            &KeymapEdit::Reset {
                command: "file.save".into(),
                when: Some("editorFocused".into()),
            },
        );
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 0,
                added: 0
            }
        );
        assert_eq!(user.len(), 2, "and it changed nothing");
    }

    #[test]
    fn reset_all_truncates_and_counts_what_it_threw_away() {
        let mut user = vec![
            Binding::new("ctrl+p", "-picker.files"),
            Binding::new("ctrl+shift+o", "picker.files"),
            Binding::new("f4", "-sidebar.toggle").when("shellWindow"),
        ];
        let outcome = apply_edit(&mut user, &KeymapEdit::ResetAll);
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 3,
                added: 0
            }
        );
        assert_eq!(user, Vec::new());
        // And the compiled-in table is exactly what is left, untouched by any of it.
        assert_eq!(resolve(&user), resolve(&[]));
    }

    #[test]
    fn an_edit_leaves_the_users_other_lines_where_they_were() {
        // Order in this file is semantic — `apply_layer` walks it in sequence and a removal
        // only matches what is already accumulated — so an editor that rebuilt it from a
        // sorted or regrouped copy would change what it means. Asserted as the whole file,
        // in order, because "my line is still in there somewhere" is not the property.
        let mut user = vec![
            Binding::new("ctrl+w", "-tab.close").when("tabPinned"),
            Binding::new("alt+z", "theme.toggle"),
        ];
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "git.pull".into(),
                when: None,
                key: "ctrl+alt+p".into(),
            },
        );
        assert_eq!(
            spell(&user),
            [
                "ctrl+w -tab.close when tabPinned",
                "alt+z theme.toggle",
                "ctrl+t -git.pull",
                "ctrl+alt+p git.pull",
            ]
        );
    }

    /// **Every default can be moved to a free chord without manufacturing a conflict.**
    ///
    /// The sweep the individual cases above are examples of. For each shipped binding: rebind
    /// its command onto a key nothing uses, and demand three things of the result — the
    /// command answers to the new chord and only the new chord, the old chord is free, and
    /// `conflicts` is silent. A rebind that forgot the removal passes the first, fails the
    /// second and reports a conflict on the third; one that dropped the `when` passes all
    /// three for the unconditional bindings and fails for the scoped ones, which is why this
    /// walks the whole table rather than a hand-picked binding.
    #[test]
    fn every_default_binding_can_be_moved_without_leaving_the_old_one_behind() {
        for binding in defaults() {
            let mut user = Vec::new();
            let target = "ctrl+alt+shift+meta+F9";
            apply_edit(
                &mut user,
                &KeymapEdit::Rebind {
                    command: binding.command.clone(),
                    when: binding.when.clone(),
                    key: target.into(),
                },
            );

            let resolved = resolve(&user);
            // Scoped to the (command, `when`) row the edit named, which is the identity every
            // `KeymapEdit` carries: since the Alt+Arrow round, `pane.navigate.up`/`.down` each
            // stand on two rows with *different* clauses (`ctrl+alt+k` unscoped, `alt+up` as
            // the `!editorFocused` complement of the member walk), and moving one row must not
            // drag the other with it.
            let keys: Vec<&str> = resolved
                .iter()
                .filter(|r| r.command == binding.command && r.when == binding.when)
                .map(|r| r.key.as_str())
                .collect();
            assert_eq!(
                keys,
                [normalize_key(target)],
                "{} ({:?}) should answer to one chord after a rebind",
                binding.command,
                binding.when
            );
            // …and the sibling row, where the command has one, is exactly where it shipped.
            let siblings: Vec<String> = resolved
                .iter()
                .filter(|r| r.command == binding.command && r.when != binding.when)
                .map(|r| r.key.clone())
                .collect();
            let shipped: Vec<String> = defaults()
                .iter()
                .filter(|o| o.command == binding.command && o.when != binding.when)
                .map(|o| normalize_key(&o.key))
                .collect();
            assert_eq!(
                siblings, shipped,
                "rebinding {} ({:?}) touched the command's other scope",
                binding.command, binding.when
            );
            assert_eq!(
                conflicts(&resolved),
                Vec::new(),
                "rebinding {} manufactured a conflict",
                binding.command
            );
            let (_, diags) = resolve_with_diagnostics(&user);
            assert!(
                diags.is_empty(),
                "rebinding {} wrote a removal that matched nothing: {diags:?}",
                binding.command
            );

            // …and *Restore default* puts it back exactly as it shipped.
            apply_edit(
                &mut user,
                &KeymapEdit::Reset {
                    command: binding.command.clone(),
                    when: binding.when.clone(),
                },
            );
            assert_eq!(user, Vec::new(), "reset must empty the file it filled");
            assert_eq!(resolve(&user), resolve(&[]));
        }
    }

    /// Every default can be unbound, and unbinding is the only thing it does.
    ///
    /// Pinned to the PC layer, and the assertion at the foot is *why*: an unbind must write one
    /// removal line **per chord the (command, `when`) row actually stands on**, and not one
    /// more. Fewer would leave the row live on a chord the user thought they had taken away —
    /// the precise failure `suppress` exists to prevent, and the one the macOS test below
    /// states for the layer that adds a second chord.
    ///
    /// It read `assert_eq!(user.len(), 1)` until M19, with a comment calling one-chord-per-command
    /// a property of `defaults()`. That was true and incidental rather than intended, and folding
    /// ended it on purpose: `ctrl+equal` and `ctrl+plus` are **two physical keys** — the main row's
    /// `=` and the keypad's `+`, which `keys/chords.ts` names `equal` and `plus` off
    /// `KeyboardEvent.code` — for one glyph and one command. Spelling only one of them would make
    /// Expand dead on the numeric keypad while Collapse worked there, which is worse than either
    /// alternative. So the counted form is the invariant, and it was always the one that mattered.
    #[test]
    fn every_default_binding_can_be_given_back_to_whatever_is_underneath() {
        for binding in defaults() {
            let mut user = Vec::new();
            edit_pc(
                &mut user,
                &KeymapEdit::Unbind {
                    command: binding.command.clone(),
                    when: binding.when.clone(),
                },
            );
            let resolved = resolve_pc(&user);
            // The row is the (command, `when`) pair — the identity every `KeymapEdit` carries —
            // and not the bare command: `pane.navigate.up`/`.down` stand on two rows with
            // different clauses since the Alt+Arrow round, and unbinding the unscoped chord
            // must leave the `!editorFocused` one standing, exactly as the editor's screen
            // shows them as two lines.
            assert!(
                !resolved
                    .iter()
                    .any(|r| r.command == binding.command && r.when == binding.when),
                "{} ({:?}) is still bound after an unbind",
                binding.command,
                binding.when
            );
            let (_, diags) = resolve_with_diagnostics_pc(&user);
            assert!(
                diags.is_empty(),
                "unbinding {} matched nothing: {diags:?}",
                binding.command
            );
            // One line per chord the command stands on, and not one more, so a user's file
            // stays readable and no chord survives the unbind.
            let chords = defaults()
                .iter()
                .filter(|other| other.command == binding.command && other.when == binding.when)
                .count();
            assert_eq!(
                user.len(),
                chords,
                "unbinding {} wrote {} lines for {chords} chord(s)",
                binding.command,
                user.len()
            );
        }
    }

    /// On macOS an unbind of `navigate.back` writes **two** removals, and that is correct.
    ///
    /// The macOS layer does not only rewrite; it *adds* — `meta+bracketleft`/`meta+bracketright`
    /// go on top of the `mouseback`/`mouseforward` defaults, which carry no `ctrl` and so are
    /// left alone by the rewrite loop. So `navigate.back` stands on two chords there, and
    /// "unbound" has to mean both of them: suppressing one would leave the command live on the
    /// other, which is the precise failure `suppress` exists to prevent.
    ///
    /// Worth its own test because the invariant above ("exactly one line per unbind") reads like
    /// a property of the editor and is really a property of `defaults()`. Discovered by running
    /// the suite against the macOS layer, where it was one of sixteen failures — the only one
    /// that was not merely a hard-coded `ctrl+` spelling.
    #[test]
    fn an_unbind_on_macos_takes_away_both_chords_the_layer_gave() {
        let mac = platform_layer(true);

        // The premise, asserted rather than assumed: two chords, not one.
        let standing: Vec<String> = resolve_layers(&mac, &[])
            .into_iter()
            .filter(|r| r.command == "navigate.back")
            .map(|r| r.key)
            .collect();
        assert_eq!(
            standing,
            ["mouseback", "meta+bracketleft"],
            "the macOS layer adds ⌘[ beside the thumb button rather than replacing it"
        );

        let mut user = Vec::new();
        let outcome = apply_edit_layered(
            &mac,
            &mut user,
            &KeymapEdit::Unbind {
                command: "navigate.back".into(),
                when: None,
            },
        );
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 0,
                added: 2
            }
        );
        assert_eq!(
            spell(&user),
            [
                "mouseback -navigate.back",
                "meta+bracketleft -navigate.back"
            ]
        );
        assert!(
            !resolve_layers(&mac, &user)
                .iter()
                .any(|r| r.command == "navigate.back"),
            "a command left standing on its second chord is not unbound"
        );

        // And the PC layer still answers with one line, which is what the test above pins.
        let mut pc = Vec::new();
        edit_pc(
            &mut pc,
            &KeymapEdit::Unbind {
                command: "navigate.back".into(),
                when: None,
            },
        );
        assert_eq!(spell(&pc), ["mouseback -navigate.back"]);
    }

    #[test]
    fn a_command_with_no_default_is_bound_by_a_single_line() {
        // Roughly half the registry ships unbound — `file.saveAll` among them — and those are
        // the rows a keymap editor exists for. There is nothing underneath to suppress, so
        // there is nothing to remove.
        let mut user = Vec::new();
        let outcome = apply_edit(
            &mut user,
            &KeymapEdit::Rebind {
                command: "file.saveAll".into(),
                when: None,
                key: "ctrl+alt+shift+s".into(),
            },
        );
        assert_eq!(
            outcome,
            EditOutcome {
                removed: 0,
                added: 1
            }
        );
        assert_eq!(spell(&user), ["ctrl+alt+shift+s file.saveAll"]);
        assert_eq!(
            command_for(&resolve(&user), "ctrl+alt+shift+s"),
            ["file.saveAll"]
        );
    }

    #[test]
    fn a_key_is_stored_normalised_however_it_was_written() {
        // The screen sends what the recorder captured, which is canonical — but the command is
        // reachable from anywhere and `keymap.json` is compared on normalised keys, so an
        // edit that stored `Shift+Ctrl+P` would produce an override that never shadows the
        // default it was aimed at.
        let mut user = Vec::new();
        apply_edit(
            &mut user,
            &KeymapEdit::Rebind {
                command: "file.saveAll".into(),
                when: None,
                key: "Shift+CTRL+Alt+S".into(),
            },
        );
        assert_eq!(spell(&user), ["ctrl+alt+shift+s file.saveAll"]);
    }

    /// The one asymmetry between the two normalisers, pinned rather than discovered.
    ///
    /// `normalize_key` here deliberately renames no keys, and the frontend's `chords.ts` folds
    /// `` ` ``, `~`, `tilde` and `grave` onto `backquote`. So a chord recorded in the webview
    /// arrives spelled `ctrl+backquote` while the default it is displacing is written
    /// `` ctrl+` ``, and to *this* module those are two different keys. The rebind still comes
    /// out right — the removal names the key the resolution reports, not the one the caller
    /// typed — and this pins that, because the shape of the file is surprising enough to be
    /// mistaken for a bug: it names both spellings.
    #[test]
    fn a_chord_recorded_in_the_webview_still_displaces_the_default_it_lands_on() {
        let mut user = Vec::new();
        edit_pc(
            &mut user,
            &KeymapEdit::Rebind {
                command: "picker.files".into(),
                when: None,
                key: "ctrl+backquote".into(),
            },
        );
        assert_eq!(
            spell(&user),
            ["ctrl+p -picker.files", "ctrl+backquote picker.files"]
        );

        let resolved = resolve_pc(&user);
        assert!(command_for(&resolved, "ctrl+p").is_empty());
        assert_eq!(command_for(&resolved, "ctrl+backquote"), ["picker.files"]);
        // The project switcher is still on its own spelling of the same physical key, and
        // this module cannot see that they are the same key. `ui/src/keys/keymapModel.ts`
        // can, and warns before the write — which is why the warning lives there.
        assert_eq!(
            command_for(&resolved, "ctrl+`"),
            ["project.switcher.next"],
            "Rust compares written spellings; the alias table is the frontend's"
        );
    }

    #[test]
    fn only_a_lone_reset_all_may_overwrite_a_file_that_does_not_parse() {
        let reset = KeymapEdit::ResetAll;
        let rebind = KeymapEdit::Rebind {
            command: "file.save".into(),
            when: None,
            key: "ctrl+alt+s".into(),
        };

        assert!(may_replace_unreadable(std::slice::from_ref(&reset)));
        assert!(!may_replace_unreadable(&[]));
        assert!(!may_replace_unreadable(std::slice::from_ref(&rebind)));
        // Not even smuggled in beside one: the batch is applied to a vector that would have
        // started empty, so the rebind's own removals would be aimed at nothing and the
        // user's other lines would be gone.
        assert!(!may_replace_unreadable(&[
            KeymapEdit::ResetAll,
            rebind.clone()
        ]));
        assert!(!may_replace_unreadable(&[rebind, KeymapEdit::ResetAll]));
    }

    #[test]
    fn save_user_round_trips_through_load_user() {
        let dir = TempDir::new();
        let path = dir.0.join("keymap.json");
        let mut user = Vec::new();
        apply_edit(
            &mut user,
            &KeymapEdit::Rebind {
                command: "sidebar.toggle".into(),
                when: Some("shellWindow".into()),
                key: "f6".into(),
            },
        );
        // An **unconditional** rebind beside the scoped one, and that is the half that
        // measures anything: `when` and `args` are `Option`, and a file whose every entry
        // happens to carry a `when` cannot tell whether the empty ones are being skipped.
        apply_edit(
            &mut user,
            &KeymapEdit::Rebind {
                command: "file.save".into(),
                when: None,
                key: "ctrl+alt+s".into(),
            },
        );

        save_user(&path, &user).expect("write");
        assert_eq!(load_user(&path).expect("read back"), user);

        // No `"when": null, "args": null` on every line: this is a file people read.
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            !text.contains("null"),
            "a rewritten file should stay legible:\n{text}"
        );
        assert!(text.ends_with("]\n"), "and end with a newline:\n{text}");
    }

    #[test]
    fn save_user_keeps_the_previous_contents_beside_the_file() {
        // The honest half of "saving reflows your file": whatever was there is one `mv` away.
        let dir = TempDir::new();
        let path = dir.write(
            "keymap.json",
            "[{\"key\":\"ctrl+t\",\"command\":\"-git.pull\"}]",
        );
        save_user(&path, &[Binding::new("f6", "sidebar.toggle")]).expect("write");

        let backup = std::fs::read_to_string(dir.0.join("keymap.json.bak")).expect("backup");
        assert_eq!(backup, "[{\"key\":\"ctrl+t\",\"command\":\"-git.pull\"}]");
        assert_eq!(
            load_user(&path).expect("read"),
            vec![Binding::new("f6", "sidebar.toggle")]
        );
    }

    #[test]
    fn save_user_creates_the_file_and_its_directory() {
        let dir = TempDir::new();
        let path = dir.0.join("nested").join("keymap.json");
        save_user(&path, &[]).expect("write into a directory that does not exist yet");
        assert_eq!(load_user(&path).expect("read"), Vec::new());
        assert!(
            !dir.0.join("nested").join("keymap.json.bak").exists(),
            "there was nothing to back up"
        );
    }

    /// A `keymap.json` symlinked into a dotfiles repository stays a symlink.
    ///
    /// That arrangement is how a hand-authored keymap usually reaches a machine, and a plain
    /// rename over the link would replace it with a regular file — leaving the repository copy
    /// stale and the user's next `git status` clean while their edits went somewhere else.
    #[cfg(unix)]
    #[test]
    fn save_user_writes_through_a_symlink_rather_than_replacing_it() {
        let dir = TempDir::new();
        let real = dir.write("dotfiles-keymap.json", "[]");
        let link = dir.0.join("keymap.json");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        save_user(&link, &[Binding::new("f6", "sidebar.toggle")]).expect("write");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("stat")
                .file_type()
                .is_symlink(),
            "the link must survive the write"
        );
        assert_eq!(
            load_user(&real).expect("read the real file"),
            vec![Binding::new("f6", "sidebar.toggle")]
        );
    }

    #[test]
    fn load_user_treats_a_missing_file_as_no_overrides() {
        let dir = TempDir::new();
        let bindings = load_user(&dir.0.join("keymap.json")).expect("missing file is fine");
        assert_eq!(bindings, Vec::new());
    }

    #[test]
    fn load_user_reads_the_vs_code_file_format() {
        let dir = TempDir::new();
        let path = dir.write(
            "keymap.json",
            r#"[
              { "key": "ctrl+k ctrl+s", "command": "settings.keymap" },
              { "key": "ctrl+w", "command": "-tab.close", "when": "tabPinned" }
            ]"#,
        );
        let bindings = load_user(&path).expect("valid json");
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].command, "settings.keymap");
        assert_eq!(bindings[0].when, None);
        assert!(bindings[1].is_removal());
        assert_eq!(bindings[1].target_command(), "tab.close");
    }

    #[test]
    fn load_user_reports_malformed_json_as_a_serde_error() {
        let dir = TempDir::new();
        let path = dir.write("keymap.json", r#"[ { "key": "ctrl+p", ]"#);
        let err = load_user(&path).expect_err("malformed json");
        assert!(matches!(err, CoreError::Serde(_)), "{err:?}");
    }

    #[test]
    fn load_user_treats_an_empty_file_as_no_overrides() {
        let dir = TempDir::new();
        let path = dir.write("keymap.json", "\n  \n");
        assert_eq!(load_user(&path).expect("empty file is fine"), Vec::new());
    }
}
