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
        ("ctrl+alt+h", "pane.navigate.left"),
        ("ctrl+alt+l", "pane.navigate.right"),
        ("ctrl+alt+k", "pane.navigate.up"),
        ("ctrl+alt+j", "pane.navigate.down"),
        ("ctrl+comma", "settings.open"),
    ]
    .into_iter()
    .map(|(key, command)| Binding::new(key, command))
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
