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
//! # `when` vocabulary
//!
//! Clauses use the same grammar as [`Binding::when`](cide_ipc::Binding), and the frontend
//! supplies the context flags. The vocabulary is kept deliberately small — every flag here
//! is one the webview already tracks to paint focus rings:
//!
//! * `claudePaneFocused`, `terminalFocused`, `editorFocused` — the focused pane's kind
//! * `repoOpen` — the active project has at least one git repository

use std::sync::OnceLock;

use cide_ipc::Command;

use crate::{CoreError, Result};

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
const CLAUDE: &str = "Claude";
const TERMINAL: &str = "Terminal";
const FILE: &str = "File";
const GIT: &str = "Git";
const VIEW: &str = "View";

/// Build the table. Order here is palette order.
fn build() -> Vec<Command> {
    vec![
        // Window.
        Command::new("pane.split.right", "Split pane right (adds a tile)", WINDOW),
        Command::new("pane.split.down", "Split pane down (adds a row)", WINDOW),
        Command::new("pane.promoteToTab", "Promote pane to full tab", WINDOW),
        Command::new("pane.detachToWindow", "Detach pane into window", WINDOW),
        Command::new("pane.close", "Close pane", WINDOW),
        Command::new("pane.maximize", "Maximize pane", WINDOW),
        Command::new("pane.navigate.left", "Focus pane left", WINDOW),
        Command::new("pane.navigate.right", "Focus pane right", WINDOW),
        Command::new("pane.navigate.up", "Focus pane up", WINDOW),
        Command::new("pane.navigate.down", "Focus pane down", WINDOW),
        Command::new("window.mode.stacked", "Window mode: stacked", WINDOW),
        Command::new(
            "window.mode.perProject",
            "Window mode: one window per project",
            WINDOW,
        ),
        Command::new("tab.close", "Close tab", WINDOW),
        Command::new("tab.next", "Next tab", WINDOW),
        Command::new("tab.prev", "Previous tab", WINDOW),
        // Claude. All of these act on the focused session, so none of them mean anything
        // without a Claude pane to act on.
        Command::new(
            "claude.split.newSession",
            "Split: new Claude session",
            CLAUDE,
        )
        .when("claudePaneFocused"),
        Command::new("claude.fork", "Fork session into new pane", CLAUDE).when("claudePaneFocused"),
        Command::new("claude.mirror", "Mirror session into new pane", CLAUDE)
            .when("claudePaneFocused"),
        Command::new("claude.restart", "Restart Claude session", CLAUDE).when("claudePaneFocused"),
        Command::new("claude.mention.file", "Mention file in Claude", CLAUDE)
            .when("claudePaneFocused"),
        // Terminal. Splitting one below is offered from any pane — it creates the terminal
        // it splits to — whereas clearing needs a terminal already focused.
        Command::new("terminal.splitBelow", "Split terminal below", TERMINAL),
        Command::new("terminal.clear", "Clear terminal", TERMINAL).when("terminalFocused"),
        Command::new("terminal.paste", "Paste into terminal", TERMINAL).when("terminalFocused"),
        // File.
        Command::new("file.save", "Save file", FILE).when("editorFocused"),
        Command::new("file.saveAll", "Save all files", FILE),
        Command::new("file.reveal", "Reveal file in sidebar", FILE).when("editorFocused"),
        // Git.
        Command::new("git.commit", "Commit changes", GIT).when("repoOpen"),
        Command::new("git.push", "Push to remote", GIT).when("repoOpen"),
        Command::new("git.refresh", "Refresh git status", GIT).when("repoOpen"),
        Command::new("git.stageSelected", "Stage selected changes", GIT).when("repoOpen"),
        // View.
        Command::new("picker.files", "Go to file", VIEW),
        Command::new("palette.commands", "Show all commands", VIEW),
        Command::new("theme.toggle", "Toggle light/dark theme", VIEW),
        Command::new("settings.open", "Open settings", VIEW),
        Command::new("settings.keymap", "Open keyboard shortcuts", VIEW),
        Command::new("sidebar.files", "Show files sidebar", VIEW),
        Command::new("sidebar.git", "Show git sidebar", VIEW).when("repoOpen"),
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

    #[test]
    fn every_when_clause_uses_the_documented_vocabulary() {
        // The frontend supplies these flags by name. A clause naming a flag it does not set
        // is never true, so the command vanishes from the palette on every platform and
        // nothing anywhere reports it.
        for command in registry() {
            let Some(clause) = command.when.as_deref() else {
                continue;
            };
            assert!(
                matches!(
                    clause,
                    "claudePaneFocused" | "terminalFocused" | "editorFocused" | "repoOpen"
                ),
                "{} has an undocumented when clause: {clause}",
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
            vec!["Window", "Claude", "Terminal", "File", "Git", "View"]
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
    fn claude_commands_require_a_claude_pane() {
        for command in registry().iter().filter(|c| c.id.starts_with("claude.")) {
            assert_eq!(command.when.as_deref(), Some("claudePaneFocused"));
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
