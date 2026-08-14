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

use cide_ipc::{Binding, KeymapLayer, ResolvedBinding};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, Result};

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
        ("ctrl+shift+t", "theme.toggle"),
        // Switching projects, asked for by name: "a hotkey with default CTRL+TAB to switch
        // between opened projects" — and then, when asked whether the cycle should be header
        // order or most-recently-used, answered precisely: "most-recently-used, but until
        // CTRL is pressed and pressing TAB twice - should follow to iteration between whole
        // list".
        //
        // That is the Windows / IDEA / browser switcher and nothing else: **hold** Ctrl, and
        // the popup that appears selects the previously used project, so one press-and-release
        // is the two-item toggle everyone expects; each further Tab with Ctrl still down walks
        // one further down the whole MRU list; the release commits.
        //
        // An earlier round shipped header order here, and its stated reason was that this gate
        // resolves on keydown and has no key-up path, so MRU without a hold would degenerate
        // into a toggle between two projects with the third unreachable for ever. The premise
        // was right and the conclusion was the wrong way round: the fix is the hold, not a
        // different order. It is built — `ui/src/keys/switcher.ts` is the walk, and the
        // release is watched by a modifier latch that resolves no chords, so the gate is still
        // keydown-only at both of its entry points.
        //
        // `project.next` / `project.prev` still exist and still walk the header strip. They
        // are simply not what Ctrl+Tab means any more.
        ("ctrl+tab", "project.switcher.next"),
        ("ctrl+shift+tab", "project.switcher.prev"),
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
        ("ctrl+comma", "settings.open"),
        // M12. `ctrl+f12` is IDEA's own File Structure chord, and no f-key is bound anywhere else
        // in this workspace; `ctrl+alt+shift+n` is free in every layer (`ctrl+shift+n` is
        // `claude.split.newSession`, and the two normalise to different keys).
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
        // word-motion in readline and would be taken from every terminal in every window by the
        // window capture listener. `ctrl+alt+shift+left`/`right` are free in every layer and are
        // what a user should add if they want them — `README.md` prints the two lines. Shipping
        // an unbound pair rather than guessing is the same trade `file.saveAll` already makes.
        ("mouseback", "navigate.back"),
        ("mouseforward", "navigate.forward"),
    ]
    .into_iter()
    .map(|(key, command)| Binding::new(key, command))
    // Bindings that carry a `when`, and the first ones in this table that do.
    //
    // # Why these two need a clause when nothing above does
    //
    // `alt+up` / `alt+down` are IDEA's *Previous/Next Method*. Unconditionally bound they would
    // also fire in a terminal pane, where the gesture would move a caret the user cannot see —
    // and the key gate is a window **capture** listener, so a globally-bound chord never reaches
    // the pane that should have had it.
    //
    // # What this costs, named rather than hidden
    //
    // `@codemirror/commands` binds `Alt-ArrowUp`/`Alt-ArrowDown` to `moveLineUp`/`moveLineDown`,
    // so inside a buffer those stop moving lines. `EditorSurface` re-homes them to
    // `Mod-Shift-Arrow` — IDEA's own chord for the same thing — which is free in both layers, so
    // a capability moves rather than disappearing.
    //
    // The alternative was `ctrl+alt+up`/`ctrl+alt+down`, and it costs strictly more: `ctrl+alt+down`
    // is already `pane.split.down` above, *and* CodeMirror binds `Mod-Alt-Arrow` to
    // `addCursorAbove`/`addCursorBelow`. Two breakages against this one.
    .chain(
        [
            ("alt+down", "navigate.nextMember", "editorFocused"),
            ("alt+up", "navigate.prevMember", "editorFocused"),
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
    out
}

/// Chords that stay on `ctrl` even on macOS.
///
/// The blanket ctrl→meta rewrite is right for `⌘P`, `⌘S` and the rest, and wrong for exactly
/// one thing: **Tab**. `⌘⇥` is the system application switcher — macOS consumes it before any
/// app sees it — so rewriting `ctrl+tab` there would not move the binding, it would delete
/// it, and Ctrl+Tab is what switches tabs on macOS in Safari, Chrome and VS Code anyway. This
/// is the whole exception list; it is a function rather than a `const` array so the rule is
/// stated where the reason is.
fn keeps_ctrl_on_macos(key: &str) -> bool {
    parse_chord(key)
        .map(|chords| chords.iter().any(|chord| chord.key == "tab"))
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
            "picker.files",
            "palette.commands",
            "tab.close",
            "file.save",
            "theme.toggle",
            "pane.navigate.left",
            "pane.navigate.right",
            "pane.navigate.up",
            "pane.navigate.down",
            "settings.open",
            "project.switcher.next",
            "project.switcher.prev",
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

    #[test]
    fn the_default_keymap_has_no_conflicts() {
        assert_eq!(conflicts(&resolve(&[])), Vec::new());
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
        let resolved = resolve(&user);
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
        let resolved = resolve(&user);

        // The unconditional default is a different binding and survives untouched.
        assert_eq!(command_for(&resolved, "ctrl+w"), vec!["tab.close"]);
    }

    #[test]
    fn a_removal_matching_the_same_context_does_remove() {
        let user = vec![
            Binding::new("ctrl+w", "tab.close").when("tabPinned"),
            Binding::new("ctrl+w", "-tab.close").when("tabPinned"),
        ];
        let resolved = resolve(&user);

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
        let resolved = resolve(&[Binding::new("ctrl+s", "file.saveAll")]);
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
    fn the_macos_layer_leaves_ctrl_tab_alone() {
        // `⌘⇥` is the system app switcher: macOS never delivers it, so rewriting this one
        // would silently *remove* project switching on the platform rather than move it.
        let resolved = resolve_layers(&platform_layer(true), &[]);
        assert_eq!(
            command_for(&resolved, "ctrl+tab"),
            ["project.switcher.next"]
        );
        assert_eq!(
            command_for(&resolved, "ctrl+shift+tab"),
            ["project.switcher.prev"]
        );
        assert!(
            command_for(&resolved, "meta+tab").is_empty(),
            "nothing may be bound to the macOS application switcher"
        );
        // And the exception is narrow: everything else still moves to ⌘.
        assert_eq!(command_for(&resolved, "meta+w"), ["tab.close"]);
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
    /// resolver that folded shift away would silently swap the theme on every pull.
    #[test]
    fn ctrl_t_pulls_and_can_be_given_back_to_the_terminal() {
        let shipped = resolve(&[]);
        assert_eq!(command_for(&shipped, "ctrl+t"), ["git.pull"]);
        assert_eq!(command_for(&shipped, "ctrl+shift+t"), ["theme.toggle"]);

        let freed = resolve(&[Binding::new("ctrl+t", "-git.pull")]);
        assert!(
            command_for(&freed, "ctrl+t").is_empty(),
            "the documented one-liner has to actually restore transpose-chars"
        );
        assert_eq!(
            command_for(&freed, "ctrl+shift+t"),
            ["theme.toggle"],
            "and it must not take the neighbouring chord with it"
        );
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
        let shipped = resolve(&[]);
        assert_eq!(command_for(&shipped, "ctrl+g"), ["navigate.line"]);
        // The neighbouring chord is a *different* stroke, so find-previous survives untouched —
        // which is the asymmetry the binding's own comment names.
        assert!(command_for(&shipped, "ctrl+shift+g").is_empty());

        let scoped = resolve(&[Binding::new("ctrl+g", "-navigate.line").when("editorFocused")]);
        assert!(
            command_for(&scoped, "ctrl+g").is_empty(),
            "the documented one-liner has to actually give Mod-g back to the editor"
        );

        let unscoped = resolve(&[Binding::new("ctrl+g", "-navigate.line")]);
        assert_eq!(
            command_for(&unscoped, "ctrl+g"),
            ["navigate.line"],
            "a removal that forgets the `when` matches nothing — the README must not print it"
        );
        let (_, diags) = resolve_with_diagnostics(&[Binding::new("ctrl+g", "-navigate.line")]);
        assert!(
            diags.iter().any(|d| matches!(
                d,
                KeymapDiagnostic::RemovalMatchedNothing { command, .. } if command == "navigate.line"
            )),
            "and the user is told, rather than left with a line that looks right: {diags:?}"
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
