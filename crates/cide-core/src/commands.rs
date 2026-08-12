//! The command registry — one table behind both the palette and the keymap.
//!
//! Every action a user can reach by name lives here exactly once. Two tables would let a
//! command be bindable but unlistable (a key that works and cannot be discovered) or
//! listable but unbindable (a palette entry no key can reach), and both drift silently
//! because nothing fails when they disagree. `cide-core::keymap` does not consult this
//! table at resolution time — it layers whatever ids it is given — so the two are held in
//! step by a test here that walks the compiled-in bindings and demands every id they name
//! be registered. An id typo in a default binding fails the build rather than shipping a
//! key that does nothing.
//!
//! # Ids are API
//!
//! A user's `keymap.json` refers to commands by id. Renaming one silently breaks their
//! configuration, so ids are treated as a public interface: added freely, never renamed.
//! Titles are display text and may change with the mock.
//!
//! # Listed, or deleted. Never listed and dead.
//!
//! An entry here is a promise that choosing it from the palette does something. There are
//! exactly two honest states for a command — it runs, or it is drawn disabled with a reason
//! ([`Command::unavailable`]) — and the third state, listed and silently inert, is what
//! shipped: of the 37 entries this table held, `keys/dispatch.ts` had an arm for 7 and
//! `App.tsx`'s fallback for 6 more, so **24 resolved to a diagnostic log line** — the palette
//! advertised them and the keymap swallowed their keystrokes. `ui/scripts/check-commands.mjs`
//! is the gate that makes that state unrepresentable: every id here is either handled by
//! `ui/src/keys/dispatch.ts` or carries an `unavailable` reason, and the build fails
//! otherwise.
//!
//! # `when` vocabulary
//!
//! Clauses use the same grammar as [`Binding::when`](cide_ipc::Binding), and the frontend
//! supplies the context flags. [`CONTEXT_FLAGS`] is the whole vocabulary and is the single
//! list the doc, the validation test and the frontend's own check script all read — a flag
//! named in a clause but never *set* by the webview is false for ever, which hides the
//! command from the palette on every platform with nothing anywhere reporting it. That had
//! already happened to `repoOpen`, so every git command was invisible.
//!
//! # Preconditions go in `when` **and** in the handler
//!
//! A command that no-ops because there is no focused pane is indistinguishable, from the
//! user's chair, from one that is unwired — which is the entire complaint this round
//! answers. So anything that acts on a pane, a tab, a selection or a repository says so in
//! its clause, and the palette hides it rather than offering a row that does nothing.
//!
//! A clause here does **not** gate the keyboard, and that is worth stating plainly because
//! the shape invites the opposite assumption. The key gate resolves a chord through
//! [`Binding::when`](cide_ipc::Binding) — a different field, `None` on every entry in
//! [`crate::keymap::defaults`] — and never consults the command it dispatches to. So the
//! clause is the palette's filter, and the handler in `ui/src/keys/dispatch.ts` re-checks
//! the same fact for the key path, deriving it from the same place the flag is derived from.
//! Leaving that second check out is how Ctrl+W on the pinned console came to raise a
//! confirmation dialog and then fail. Merging the two fields instead was the alternative and
//! it loses: `file.save` is `editorFocused`, and a Ctrl+S the gate stopped swallowing would
//! reach a focused terminal as `^S`, which is flow control, and freeze the pane.

use std::sync::OnceLock;

use cide_ipc::Command;

use crate::{CoreError, Result};

/// Every context flag a `when` clause here — or in a user's `keymap.json` — may name.
///
/// The frontend must set each of these. `ui/src/keys/context.ts` derives all but the four
/// marked *host*, which only React knows, and `check-commands.mjs` asserts the two lists
/// cover this one. An unknown identifier evaluates to `false` rather than erroring, which is
/// the safe direction for a user's typo and the dangerous one for ours — hence the list.
pub const CONTEXT_FLAGS: &[&str] = &[
    // The focused pane, and whether there is one at all.
    "paneFocused",
    "claudePaneFocused",
    "terminalFocused",
    "editorFocused",
    // The workspace around it.
    "projectOpen",
    "multipleProjects",
    "multipleTabs",
    "closableTab",
    "editorOpen",
    "claudeTarget",
    "repoOpen",
    "shellWindow",
    // Host flags: transient chrome that only the React tree knows about.
    "overlayOpen",
    "contextMenuOpen",
    "sidebarFiles",
    "sidebarGit",
];

/// Every command cide can run, in palette display order.
///
/// Built once on first call and never mutated, so callers can hold `&'static` references
/// into it for the life of the process. `Bootstrap` owns its `Vec<Command>` and so clones
/// from here; nothing else needs to.
pub fn registry() -> &'static [Command] {
    static REGISTRY: OnceLock<Vec<Command>> = OnceLock::new();
    REGISTRY.get_or_init(build).as_slice()
}

/// The command with this exact id, or `None` if nothing is registered under it.
///
/// A linear scan: the table is a few dozen entries and lookups happen on keystrokes a
/// human made, so a map would buy nothing and cost a second structure to keep in step.
pub fn by_id(id: &str) -> Option<&'static Command> {
    registry().iter().find(|command| command.id == id)
}

/// The distinct groups, in the order the palette should show them.
///
/// Order comes from first appearance in the table rather than sorting, so the grouping
/// the table was written with is the grouping the user sees.
pub fn groups() -> Vec<&'static str> {
    let mut ordered: Vec<&'static str> = Vec::new();
    for command in registry() {
        if !ordered.contains(&command.group.as_str()) {
            ordered.push(command.group.as_str());
        }
    }
    ordered
}

/// Commands matching `query`, best match first.
///
/// An empty or whitespace-only query lists the whole table, which is what the palette
/// shows before the user types.
pub fn search(query: &str) -> Vec<&'static Command> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return registry().iter().collect();
    }

    let mut hits: Vec<(Score, &'static Command)> = registry()
        .iter()
        .filter_map(|command| score(command, &needle).map(|score| (score, command)))
        .collect();

    // A stable sort leaves equally scored commands in table order, so the palette does not
    // reshuffle related entries between keystrokes.
    hits.sort_by(|(a, _), (b, _)| b.tier.cmp(&a.tier).then(a.offset.cmp(&b.offset)));
    hits.into_iter().map(|(_, command)| command).collect()
}

/// Check the shipped table: no duplicate ids, no empty fields.
///
/// Meant to run at start-up rather than be trusted, because a duplicate id is a command the
/// user can bind a key to and never reach — the shadowed entry never wins a lookup. Nothing
/// in this crate calls it yet; the process entry point is where it belongs.
pub fn validate() -> Result<()> {
    validate_table(registry())
}

/// How well one command matched, as a tier plus where in the text the match landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Score {
    tier: u8,
    /// Byte offset of the match. Earlier matches rank first within a tier, so `Close pane`
    /// beats `Detach pane into window` for `pane`. Two matches at the same offset keep
    /// table order; the tier and the offset are the whole of the ranking.
    offset: usize,
}

/// Ranking tiers, best first. The palette is judged on whether typing `split` puts `Split
/// pane right` at the top, so a prefix of the title has to outrank everything else.
const TIER_TITLE_PREFIX: u8 = 5;
const TIER_TITLE_WORD: u8 = 4;
const TIER_TITLE_SUBSTRING: u8 = 3;
const TIER_ID: u8 = 2;
const TIER_SUBSEQUENCE: u8 = 1;

/// Rank one command against an already-lowercased, already-trimmed needle.
fn score(command: &Command, needle: &str) -> Option<Score> {
    let title = command.title.to_lowercase();

    if title.starts_with(needle) {
        return Some(Score {
            tier: TIER_TITLE_PREFIX,
            offset: 0,
        });
    }
    if let Some(offset) = word_prefix(&title, needle) {
        return Some(Score {
            tier: TIER_TITLE_WORD,
            offset,
        });
    }
    if let Some(offset) = title.find(needle) {
        return Some(Score {
            tier: TIER_TITLE_SUBSTRING,
            offset,
        });
    }
    if let Some(offset) = command.id.to_lowercase().find(needle) {
        return Some(Score {
            tier: TIER_ID,
            offset,
        });
    }
    if is_subsequence(&title, needle) {
        return Some(Score {
            tier: TIER_SUBSEQUENCE,
            offset: 0,
        });
    }
    None
}

/// Offset of `needle` where it starts a word in `haystack`.
///
/// Words break on anything non-alphanumeric, which covers the spaces and the colon in
/// titles like `Split: new Claude session` — typing `new` should find it.
fn word_prefix(haystack: &str, needle: &str) -> Option<usize> {
    let mut prev: Option<char> = None;
    for (offset, ch) in haystack.char_indices() {
        let at_word_start = prev.is_none_or(|p| !p.is_alphanumeric());
        if at_word_start && haystack[offset..].starts_with(needle) {
            return Some(offset);
        }
        prev = Some(ch);
    }
    None
}

/// Whether `needle`'s characters appear in `haystack` in order, gaps allowed.
///
/// This is the whole of the fuzzy matching: `spr` finds `Split pane right`. A real scorer
/// arrives with `nucleo` in M8 for the file picker, where the candidate set is a repository
/// rather than forty strings.
fn is_subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|wanted| chars.any(|ch| ch == wanted))
}

/// The shared body of [`validate`], split out so tests can point it at a malformed table
/// without mutating the static one.
fn validate_table(table: &[Command]) -> Result<()> {
    let mut seen: Vec<&str> = Vec::with_capacity(table.len());
    for command in table {
        if command.id.trim().is_empty() {
            return Err(CoreError::Invariant("a command has an empty id".to_owned()));
        }
        if command.title.trim().is_empty() {
            return Err(CoreError::Invariant(format!(
                "command {} has an empty title",
                command.id
            )));
        }
        if command.group.trim().is_empty() {
            return Err(CoreError::Invariant(format!(
                "command {} has an empty group",
                command.id
            )));
        }
        // An empty reason renders as a greyed row with nothing beside it, which says less
        // than the enabled row it replaced.
        if command
            .unavailable
            .as_ref()
            .is_some_and(|reason| reason.trim().is_empty())
        {
            return Err(CoreError::Invariant(format!(
                "command {} is unavailable with an empty reason",
                command.id
            )));
        }
        if seen.contains(&command.id.as_str()) {
            return Err(CoreError::Invariant(format!(
                "duplicate command id: {}",
                command.id
            )));
        }
        seen.push(&command.id);
    }
    Ok(())
}

/// Group names, as constants so a typo cannot split one group into two.
const WINDOW: &str = "Window";
const PROJECT: &str = "Project";
const CLAUDE: &str = "Claude";
const TERMINAL: &str = "Terminal";
const FILE: &str = "File";
const GIT: &str = "Git";
const VIEW: &str = "View";

/// Build the table. Order here is palette order.
fn build() -> Vec<Command> {
    vec![
        // Window. Everything that acts on the focused pane is gated on there being one:
        // a detached-pane window and a window whose project has just closed both have none,
        // and a row that splits nothing is the dead command this round is about.
        Command::new("pane.split.right", "Split pane right (adds a tile)", WINDOW)
            .when("paneFocused"),
        Command::new("pane.split.down", "Split pane down (adds a row)", WINDOW).when("paneFocused"),
        Command::new("pane.promoteToTab", "Promote pane to full tab", WINDOW)
            .when("paneFocused")
            // The domain has no move-pane-to-its-own-tab operation — `contract/commands.json`
            // has `pane_split`, `pane_add_row`, `pane_close` and `window_detach_pane`, and
            // nothing that re-parents a leaf into a new tab. The frontend cannot fake it:
            // closing the pane and opening a tab would kill the session the pane is holding.
            .unavailable("needs a domain command that moves a pane into a new tab"),
        Command::new("pane.detachToWindow", "Detach pane into window", WINDOW).when("paneFocused"),
        Command::new("pane.close", "Close pane", WINDOW).when("paneFocused"),
        Command::new("pane.maximize", "Maximize pane", WINDOW).when("paneFocused"),
        Command::new("pane.navigate.left", "Focus pane left", WINDOW).when("paneFocused"),
        Command::new("pane.navigate.right", "Focus pane right", WINDOW).when("paneFocused"),
        Command::new("pane.navigate.up", "Focus pane up", WINDOW).when("paneFocused"),
        Command::new("pane.navigate.down", "Focus pane down", WINDOW).when("paneFocused"),
        Command::new("window.mode.stacked", "Window mode: stacked", WINDOW),
        Command::new(
            "window.mode.perProject",
            "Window mode: one window per project",
            WINDOW,
        ),
        // `tabs[0]` is the pinned console and `close_tab` refuses it outright, so Ctrl+W on
        // it is a refusal with a dialog, not a close. The clause keeps the palette from
        // offering it — and only the palette: a clause here never reaches the keyboard,
        // because the gate resolves `Binding::when`, which is `None` on every default. The
        // handler in `keys/dispatch.ts` therefore re-checks the same fact through the same
        // function `keys/context.ts` derives this flag from.
        Command::new("tab.close", "Close tab", WINDOW).when("closableTab"),
        Command::new("tab.next", "Next tab", WINDOW).when("multipleTabs"),
        Command::new("tab.prev", "Previous tab", WINDOW).when("multipleTabs"),
        // Project. Ctrl+Tab and Ctrl+Shift+Tab; see `keymap::defaults` for why the order is
        // the header's and not most-recently-used.
        Command::new("project.next", "Next project", PROJECT).when("multipleProjects"),
        Command::new("project.prev", "Previous project", PROJECT).when("multipleProjects"),
        // Claude. `fork`, `mirror` and `restart` act on the focused *session*, so none of
        // them mean anything without a Claude pane to act on.
        //
        // `claude.split.newSession` is not one of those: it creates the session it splits to,
        // exactly as `terminal.splitBelow` creates its shell, and `ctrl+shift+n` has always
        // done that from any pane. Requiring `claudePaneFocused` did not stop the key — no
        // clause here does — it only hid the palette row in the state where the key still
        // worked, which is the palette and the keyboard disagreeing about one command.
        Command::new(
            "claude.split.newSession",
            "Split: new Claude session",
            CLAUDE,
        )
        .when("paneFocused"),
        Command::new("claude.fork", "Fork session into new pane", CLAUDE).when("claudePaneFocused"),
        Command::new("claude.mirror", "Mirror session into new pane", CLAUDE)
            .when("claudePaneFocused"),
        Command::new("claude.restart", "Restart Claude session", CLAUDE)
            .when("claudePaneFocused")
            // A restart is kill-then-respawn, and only the pane can respawn: `TerminalPane`
            // spawns once on mount and `paneHosts` caches the session id for the life of the
            // host, so killing from here leaves an exit marker and no way back. Killing alone
            // would be a *stop* command wearing the word restart.
            .unavailable("needs a respawn path in the pane host; kill alone is not a restart"),
        // The exception to the rule above, and the reason the rule is not a blanket one: you
        // mention the file you are *looking at*, which means an editor has focus and a Claude
        // pane by definition does not. `claudeTarget` is "this project has a Claude pane to
        // mention into" — the focused one, or the console.
        Command::new("claude.mention.file", "Mention file in Claude", CLAUDE)
            .when("editorFocused && claudeTarget"),
        // Terminal. Splitting one below is offered from any pane — it creates the terminal
        // it splits to — whereas clearing needs a terminal already focused.
        Command::new("terminal.splitBelow", "Split terminal below", TERMINAL).when("paneFocused"),
        Command::new("terminal.clear", "Clear terminal", TERMINAL).when("terminalFocused"),
        Command::new("terminal.paste", "Paste into terminal", TERMINAL).when("terminalFocused"),
        // File.
        Command::new("file.save", "Save file", FILE).when("editorFocused"),
        Command::new("file.saveAll", "Save all files", FILE).when("editorOpen"),
        Command::new("file.reveal", "Reveal file in sidebar", FILE).when("editorFocused"),
        // Git.
        Command::new("git.commit", "Commit changes", GIT)
            .when("repoOpen")
            // A commit needs a message and a set of paths, and both live in the Git panel's
            // React state (`useGitPanel`) — there is no store a dispatcher can read them
            // from. Committing with an empty message and every changed file would not be
            // this command, it would be a different and much worse one.
            .unavailable(
                "needs the Git panel's message and ticked paths, which are not in a store",
            ),
        Command::new("git.push", "Push to remote", GIT).when("repoOpen"),
        Command::new("git.refresh", "Refresh git status", GIT).when("repoOpen"),
        Command::new("git.stageSelected", "Stage selected changes", GIT)
            .when("repoOpen")
            // Same reason as `git.commit`: "selected" is the panel's tick state.
            .unavailable("needs the Git panel's selection, which is not in a store"),
        // View.
        Command::new("picker.files", "Go to file", VIEW).when("projectOpen"),
        Command::new("palette.commands", "Show all commands", VIEW),
        Command::new("theme.toggle", "Toggle light/dark theme", VIEW),
        // Settings opens as a *tab inside a project*, so with no project open there is
        // nowhere to put it.
        Command::new("settings.open", "Open settings", VIEW).when("projectOpen"),
        Command::new("settings.keymap", "Open keyboard shortcuts", VIEW).when("projectOpen"),
        // The rail and its sidebar exist in the shell window only; a detached pane has none.
        Command::new("sidebar.files", "Show files sidebar", VIEW).when("shellWindow"),
        Command::new("sidebar.git", "Show git sidebar", VIEW).when("shellWindow && repoOpen"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(found: &[&'static Command]) -> Vec<&'static str> {
        found.iter().map(|command| command.id.as_str()).collect()
    }

    #[test]
    fn the_shipped_registry_is_valid() {
        assert_eq!(validate(), Ok(()));
    }

    #[test]
    fn no_two_commands_share_an_id() {
        let mut seen: Vec<&str> = Vec::new();
        for command in registry() {
            assert!(
                !seen.contains(&command.id.as_str()),
                "duplicate id: {}",
                command.id
            );
            seen.push(&command.id);
        }
    }

    #[test]
    fn every_command_has_a_non_empty_title_and_group() {
        for command in registry() {
            assert!(!command.title.is_empty(), "{} has no title", command.id);
            assert!(!command.group.is_empty(), "{} has no group", command.id);
        }
    }

    #[test]
    fn by_id_round_trips_every_registered_command() {
        for command in registry() {
            let found = by_id(&command.id).expect("registered command is findable by its id");
            assert_eq!(found, command);
        }
    }

    #[test]
    fn by_id_rejects_an_unknown_id() {
        assert!(by_id("pane.split.sideways").is_none());
        // Ids are exact, not prefixes: a truncated id must not resolve to a longer one.
        assert!(by_id("pane.split").is_none());
    }

    #[test]
    fn the_ids_the_keymap_binds_are_all_registered() {
        // Read from the keymap itself rather than a copied list: a hand-maintained copy
        // agrees with the keymap only until someone adds a binding, which is the moment the
        // check exists for. Both layers, so the macOS spellings are covered off a Linux
        // host as well.
        let bound = crate::keymap::defaults()
            .into_iter()
            .chain(crate::keymap::platform_defaults());
        for binding in bound {
            let id = binding.target_command();
            assert!(
                by_id(id).is_some(),
                "{id} is bound by {} but not registered",
                binding.key
            );
        }
    }

    /// Every identifier in a `when` clause, ignoring `!`, `&&`, `||` and parentheses.
    ///
    /// The clauses are compound now (`editorFocused && claudeTarget`), so matching whole
    /// strings against a list — which is what this test used to do — would have to grow an
    /// entry per combination and would pass a clause built from two unknown flags.
    fn flags_in(clause: &str) -> Vec<String> {
        clause
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_when_clause_uses_the_documented_vocabulary() {
        // The frontend supplies these flags by name. A clause naming a flag it does not set
        // is never true, so the command vanishes from the palette on every platform and
        // nothing anywhere reports it — which is exactly what had happened to `repoOpen`,
        // and with it to every git command.
        for command in registry() {
            let Some(clause) = command.when.as_deref() else {
                continue;
            };
            for flag in flags_in(clause) {
                assert!(
                    CONTEXT_FLAGS.contains(&flag.as_str()),
                    "{} names {flag}, which is not in CONTEXT_FLAGS: {clause}",
                    command.id
                );
            }
        }
    }

    #[test]
    fn the_vocabulary_has_no_duplicates() {
        let mut seen: Vec<&str> = Vec::new();
        for flag in CONTEXT_FLAGS {
            assert!(!seen.contains(flag), "duplicate context flag: {flag}");
            seen.push(flag);
        }
    }

    #[test]
    fn nothing_binds_a_key_to_an_unavailable_command() {
        // The point of `unavailable` is that the palette can *say* why. A key bound to one
        // has no way to say anything: the gate swallows the keystroke — so the editor and
        // the pty never see it either — and the user gets silence, which is the failure this
        // whole round is about. Leave the chord unbound and it at least reaches the surface
        // underneath.
        let bound = crate::keymap::defaults()
            .into_iter()
            .chain(crate::keymap::platform_defaults());
        for binding in bound {
            let id = binding.target_command();
            let Some(command) = by_id(id) else { continue };
            assert!(
                command.unavailable.is_none(),
                "{id} is bound by {} but is unavailable: {:?}",
                binding.key,
                command.unavailable
            );
        }
    }

    #[test]
    fn an_unavailable_reason_names_what_is_missing() {
        // Read as a whole sentence in the palette: "Restart Claude session — needs a respawn
        // path in the pane host". A reason that just says "not implemented" tells a user
        // nothing they had not already worked out from the row being grey.
        for command in registry() {
            let Some(reason) = command.unavailable.as_deref() else {
                continue;
            };
            assert!(
                reason.len() > 20 && reason.contains("needs"),
                "{}'s reason should say what is missing: {reason}",
                command.id
            );
        }
    }

    #[test]
    fn no_two_commands_share_a_title() {
        // Ids keep the table honest, but the palette shows titles: two rows reading `Close
        // pane` are two rows a user cannot tell apart.
        let mut seen: Vec<&str> = Vec::new();
        for command in registry() {
            assert!(
                !seen.contains(&command.title.as_str()),
                "duplicate title: {}",
                command.title
            );
            seen.push(&command.title);
        }
    }

    /// The mock's titles, except for the two the rows model deliberately re-worded.
    ///
    /// `pane.split.right` and `pane.split.down` name two different gestures now — a tile in
    /// this row, a full-width row below it — and the mock's bare "Split pane right/down" gave
    /// a user no way to tell which was which. That is the whole complaint this milestone
    /// answers, so the divergence is the point rather than drift; the other four are still
    /// the mock's own strings.
    #[test]
    fn titles_are_the_mocks_except_the_two_the_rows_model_renamed() {
        for (id, title) in [
            ("pane.split.right", "Split pane right (adds a tile)"),
            ("pane.split.down", "Split pane down (adds a row)"),
            ("claude.split.newSession", "Split: new Claude session"),
            ("pane.promoteToTab", "Promote pane to full tab"),
            ("pane.detachToWindow", "Detach pane into window"),
            ("terminal.splitBelow", "Split terminal below"),
        ] {
            let command = by_id(id).expect("mock command is registered");
            assert_eq!(command.title, title);
        }
    }

    #[test]
    fn groups_are_listed_once_each_in_table_order() {
        assert_eq!(
            groups(),
            vec![
                "Window", "Project", "Claude", "Terminal", "File", "Git", "View"
            ]
        );
    }

    #[test]
    fn every_command_belongs_to_a_listed_group() {
        let groups = groups();
        for command in registry() {
            assert!(
                groups.contains(&command.group.as_str()),
                "{} is in unlisted group {}",
                command.id,
                command.group
            );
        }
    }

    #[test]
    fn claude_commands_require_a_claude_pane_except_the_two_that_do_not_act_on_a_session() {
        for command in registry().iter().filter(|c| c.id.starts_with("claude.")) {
            let expected = match command.id.as_str() {
                // "Mention the file I am looking at", which means an editor has focus and so
                // a Claude pane does not. Requiring `claudePaneFocused` made it a row that
                // could only ever be offered in the one state where it has no file to
                // mention.
                "claude.mention.file" => "editorFocused && claudeTarget",
                // Creates the session it splits to, so it needs a pane to split and not a
                // conversation to act on — the same clause `terminal.splitBelow` carries.
                "claude.split.newSession" => "paneFocused",
                _ => "claudePaneFocused",
            };
            assert_eq!(command.when.as_deref(), Some(expected), "{}", command.id);
        }
    }

    #[test]
    fn git_commands_require_an_open_repository() {
        for command in registry().iter().filter(|c| c.id.starts_with("git.")) {
            assert_eq!(command.when.as_deref(), Some("repoOpen"));
        }
    }

    #[test]
    fn an_empty_query_lists_the_whole_registry() {
        assert_eq!(search("").len(), registry().len());
        assert_eq!(search("   ").len(), registry().len());
    }

    #[test]
    fn searching_for_split_puts_split_pane_right_first() {
        let found = search("split");
        assert_eq!(ids(&found)[0], "pane.split.right");
        assert_eq!(
            &ids(&found)[..4],
            &[
                "pane.split.right",
                "pane.split.down",
                "claude.split.newSession",
                "terminal.splitBelow",
            ]
        );
    }

    #[test]
    fn search_is_case_insensitive() {
        assert_eq!(ids(&search("SPLIT")), ids(&search("split")));
        assert_eq!(ids(&search("  SpLiT  ")), ids(&search("split")));
    }

    #[test]
    fn a_title_prefix_outranks_a_title_substring() {
        let found = ids(&search("window"));
        let stacked = found.iter().position(|id| *id == "window.mode.stacked");
        let detach = found.iter().position(|id| *id == "pane.detachToWindow");
        assert!(
            stacked < detach,
            "`Window mode: stacked` should beat `Detach pane into window`: {found:?}"
        );
    }

    #[test]
    fn a_title_match_outranks_a_match_on_the_id_alone() {
        // `Fork session into new pane` matches only through its id, `claude.fork`.
        let found = ids(&search("claude"));
        let restart = found.iter().position(|id| *id == "claude.restart");
        let fork = found.iter().position(|id| *id == "claude.fork");
        assert!(
            restart < fork,
            "a title hit should beat an id-only hit: {found:?}"
        );
    }

    #[test]
    fn a_word_inside_the_title_is_matched() {
        let found = ids(&search("terminal"));
        assert!(found.contains(&"terminal.clear"));
        assert!(found.contains(&"terminal.splitBelow"));
    }

    #[test]
    fn initials_find_a_command_by_subsequence() {
        assert!(ids(&search("spr")).contains(&"pane.split.right"));
    }

    #[test]
    fn a_query_matching_nothing_returns_nothing() {
        assert!(search("zzqx").is_empty());
    }

    #[test]
    fn punctuation_and_non_ascii_queries_are_matched_not_feared() {
        // Everything the palette types into `search` is raw user input, and the scorer
        // slices titles by byte offset. A multi-byte character must not land it mid-char.
        assert!(!search(":").is_empty(), "titles containing a colon exist");
        assert!(search("é").is_empty());
        assert!(search("日本語").is_empty());
        assert!(search("\u{1f600}").is_empty());
        assert!(search("....").is_empty());
    }

    #[test]
    fn a_query_longer_than_any_title_matches_nothing() {
        assert!(search(&"split".repeat(20)).is_empty());
    }

    #[test]
    fn validate_rejects_a_duplicate_id() {
        let table = vec![
            Command::new("pane.close", "Close pane", WINDOW),
            Command::new("pane.close", "Shut pane", WINDOW),
        ];
        assert!(matches!(
            validate_table(&table),
            Err(CoreError::Invariant(_))
        ));
    }

    #[test]
    fn validate_rejects_an_empty_title() {
        let table = vec![Command::new("pane.close", "  ", WINDOW)];
        assert!(matches!(
            validate_table(&table),
            Err(CoreError::Invariant(_))
        ));
    }

    #[test]
    fn validate_rejects_an_empty_id_or_group() {
        let no_id = vec![Command::new("", "Close pane", WINDOW)];
        assert!(matches!(
            validate_table(&no_id),
            Err(CoreError::Invariant(_))
        ));

        let no_group = vec![Command::new("pane.close", "Close pane", "")];
        assert!(matches!(
            validate_table(&no_group),
            Err(CoreError::Invariant(_))
        ));
    }

    #[test]
    fn validate_accepts_an_empty_table() {
        assert_eq!(validate_table(&[]), Ok(()));
    }
}
