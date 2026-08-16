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
//! ## And then it happened again, to the same flag, past that gate
//!
//! `repoOpen` was given a supplier — `keys/target.ts::reposOf`, over `ProjectRoot::repo` — and
//! `check-commands.mjs` went green because the flag was now *named* by `deriveContext`. It was
//! still false for everybody. `ProjectRoot::repo` is set to `None` by
//! [`crate::workspace::project_root`] and by nothing else in the workspace, so the list was
//! empty for every project ever opened and the whole Git group stayed hidden — this time with
//! a passing gate over the top of it. A check that a flag has a supplier cannot see that the
//! supplier is a constant.
//!
//! So the rule that came out of it, and it is stronger than "every flag has a supplier":
//! **a flag must be derivable from the workspace mirror as Rust actually fills it.** Whether a
//! project contains a git repository is not such a fact — it is a property of the disk that a
//! `git init` in a bash pane changes — so it is asked over IPC (`git_repos`) by the handler
//! that needs it, and no clause claims to know it. The field it was read from has been deleted
//! (see [`cide_ipc::workspace::ProjectRoot`]) so it cannot come back by accident.
//!
//! # Preconditions go in `when` **and** in the handler
//!
//! A command that no-ops because there is no focused pane is indistinguishable, from the
//! user's chair, from one that is unwired — which is the entire complaint this round
//! answers. So anything that acts on a pane, a tab or a selection says so in its clause, and
//! the palette hides it rather than offering a row that does nothing.
//!
//! A precondition the webview cannot evaluate is the exception, and it goes in the handler
//! alone. Guessing it in the clause is how the Git group disappeared; the handler asks, and
//! answers with a sentence the user can read rather than a filtered-out row they never see.
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
    // "the tab this window is showing is *about* a file" — a file tab, or a diff tab, which
    // names the file it is diffing. Distinct from `editorFocused`, which is about the focused
    // *pane*: a file tab split with a shell pane is still a file tab, and `file.reveal` works
    // there. Derivable from the mirror as Rust actually fills it — `TabKind::File` is written
    // by `tab_open_file` on every file open — which is the rule the `repoOpen` disaster left.
    "fileTabActive",
    "claudeTarget",
    // There is deliberately no `repoOpen`. See the note above the Git group in [`build`]: the
    // webview cannot answer "does this project contain a git repository" without asking the
    // disk, and the flag it used to answer it with was permanently false.
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
///
/// The bottom three are the ones that answer "I typed a sentence and got nothing". The five
/// above them all match the needle *whole* against one string, so a query with a word the
/// title does not contain scores nothing at all — `create new branch` found neither
/// `New branch…` nor anything else, and the palette said *No matching commands* about a
/// command that exists. [`TIER_KEYWORD`] gives a command a vocabulary beyond its label and
/// [`TIER_ALL_WORDS`] lets the words be typed in any order with extra ones in between. Both
/// rank below everything that matched a real title, because both are guesses.
const TIER_TITLE_PREFIX: u8 = 6;
const TIER_TITLE_WORD: u8 = 5;
const TIER_TITLE_SUBSTRING: u8 = 4;
const TIER_ID: u8 = 3;
const TIER_KEYWORD: u8 = 2;
const TIER_ALL_WORDS: u8 = 1;
const TIER_SUBSEQUENCE: u8 = 0;

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
    // A keyword matched at a *word* boundary rather than anywhere inside it: `in files` should
    // answer to `files`, and `create` should not answer to `eat`. The tier is already the
    // vaguest kind of hit the palette offers and a substring here would make it noise.
    if let Some(offset) = command
        .keywords
        .iter()
        .find_map(|word| word_prefix(&word.to_lowercase(), needle))
    {
        return Some(Score {
            tier: TIER_KEYWORD,
            offset,
        });
    }
    if all_words_match(command, &title, needle) {
        return Some(Score {
            tier: TIER_ALL_WORDS,
            offset: 0,
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

/// Every whitespace-separated word of `needle` starts a word of the title, a keyword or the id.
///
/// Only ever consulted for a needle with two or more words: for a single word this is the
/// disjunction of three tiers that have already been tried and failed, so it could only ever
/// answer `false`, and skipping it says that in the code rather than in a comment.
///
/// Word-prefix rather than substring per word, for the reason above [`TIER_KEYWORD`] and one
/// more: with a substring, a two-letter word like `to` matches nearly every row in the table
/// and the tier stops discriminating. `create new branch` matches `New branch…` — *create*
/// from its keywords, *new* and *branch* from its title — and matches `Switch branch…` on
/// nothing, which is the distinction that makes the tier worth having.
fn all_words_match(command: &Command, title: &str, needle: &str) -> bool {
    let words: Vec<&str> = needle.split_whitespace().collect();
    if words.len() < 2 {
        return false;
    }
    let id = command.id.to_lowercase();
    let keywords: Vec<String> = command.keywords.iter().map(|w| w.to_lowercase()).collect();
    words.iter().all(|word| {
        word_prefix(title, word).is_some()
            || word_prefix(&id, word).is_some()
            || keywords.iter().any(|k| word_prefix(k, word).is_some())
    })
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
/// Moving around inside the file the caret is in. IDEA's own menu name. (M12)
const NAVIGATE: &str = "Navigate";
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
        /*
         * Ctrl+Shift+T, which is what every browser means by it — asked for by name, and it
         * took the chord back off `theme.toggle`.
         *
         * # No `when`, deliberately, and this is the interesting decision
         *
         * The obvious clause is `hasClosedTabs`, and it is the `repoOpen` trap in a new coat.
         * The stack lives in `cide_app::closed_tabs`, in Tauri state and **not** in `Workspace`
         * — a close must not bump `rev` and broadcast the whole tree for a record nothing draws,
         * the argument `positions_state.rs` already makes for view positions. So a flag for it
         * would need a supplier, and the only supplier is a field on the snapshot, and putting
         * one there is precisely the broadcast the stack was kept out of the tree to avoid.
         *
         * The alternative — a flag with no supplier — is the mistake this file records twice:
         * `repoOpen` had one, passed the gate, and was false for every user of every build,
         * which hid the whole Git group from the palette for a milestone. A row that is always
         * offered and answers "nothing to reopen" in the diagnostic log is honest; a row that is
         * never offered is not, and `check:commands` permits the first and cannot see the
         * second.
         *
         * # Why the id is `tab.reopenClosed`
         *
         * Ids are API — a user's `keymap.json` names them and they are never renamed — so it
         * says what it does rather than what key it is on. `tab.reopen` was the other candidate
         * and loses: it reads as "reload this tab", which is a different command someone will
         * want later.
         */
        Command::new("tab.reopenClosed", "Reopen closed tab", WINDOW)
            .keywords(&["undo", "restore", "closed", "last", "again"]),
        Command::new("tab.next", "Next tab", WINDOW).when("multipleTabs"),
        Command::new("tab.prev", "Previous tab", WINDOW).when("multipleTabs"),
        /*
         * The *tab* switcher, and the same split the Project group makes one screen down: an
         * immediate walk along the strip (`tab.next` / `tab.prev`, positional) and a
         * held-modifier walk in most-recently-used order (these two). Ctrl+Tab is the second
         * one, which is what every browser and every IDE means by that chord.
         *
         * It walks **tabs**, not files, and the title says so. The pinned Claude console, a
         * `ClaudeFull` tab, a file tab, a diff tab and the settings tab all live in one `Vec`,
         * and excluding the console would make the most common return trip in this app — from
         * the file you were reading back to the conversation about it — the one thing Ctrl+Tab
         * could not do. `tab.console` below is the direct route to the same place.
         *
         * `multipleTabs` and `shellWindow`. The strip lives in the shell window; a detached-tab
         * window shows exactly one tab and a detached-pane window shows none, and activating a
         * tab from either would move the *shell* window's active tab — the latent issue
         * `tab.next` / `tab.prev` above still have, which is worth naming and is not this
         * change's to fix. The default bindings carry `shellWindow` as well, so the chord is not
         * merely inert in those windows, it is not taken from them at all.
         */
        Command::new(
            "tab.switcher.next",
            "Switch tab (most recently used)",
            WINDOW,
        )
        .when("shellWindow && multipleTabs")
        .keywords(&["mru", "recent", "cycle", "file", "buffer"]),
        Command::new(
            "tab.switcher.prev",
            "Switch tab (least recently used)",
            WINDOW,
        )
        .when("shellWindow && multipleTabs")
        .keywords(&["mru", "recent", "cycle", "file", "buffer"]),
        /*
         * Ctrl+1 — the pinned console, by name rather than by position.
         *
         * The id is `tab.console` and not `tab.select.1`, because **ids are API**: a user's
         * `keymap.json` names them and they are never renamed. `tabs[0]` is a stable identity
         * that Rust enforces — `open_tab` refuses a second `ClaudeHome` and `close_tab` refuses
         * index 0 — so "the Claude console" is a thing this command can honestly promise, where
         * "tab 1" would be a position. If Ctrl+2..9 ever ship they arrive as `tab.select.2..9`
         * beside this, with nothing renamed.
         *
         * `shellWindow && projectOpen`: the console belongs to a project, and the tab strip
         * belongs to the shell window. The handler re-checks both — a clause gates the palette
         * and never the keyboard — and it goes through `revealPane`, so the gesture ends with
         * the caret in the prompt rather than in whatever the user was typing in.
         */
        Command::new("tab.console", "Go to Claude console", WINDOW)
            .when("shellWindow && projectOpen")
            .keywords(&["home", "chat", "prompt", "agent", "first"]),
        // Project. Two families, and the split is the point rather than duplication.
        //
        // The *switcher* is Ctrl+` — the key left of `1`, which is what a keyboard reports as
        // `Backquote` on every layout: hold the modifier, walk a popup in most-recently-used
        // order, release to commit. It was Ctrl+Tab until the tab switcher above took that
        // chord, and `keymap::defaults` carries both the move and the argument for why the hold
        // is not optional. Running it from the palette holds no modifier, so it degenerates to
        // "switch to the most recently used project" — one keystroke, terminating, and the
        // titles say so.
        //
        // `project.switcher.prev` ships **unbound**: its natural reverse chord,
        // `ctrl+shift+backquote`, is already `terminal.splitBelow`. It is reachable from the
        // palette, and — the way a user will actually reach it — by holding Shift while the
        // popup is up, because the switcher's capture owns its own opening key for as long as
        // it is open. See `keys/switcher.ts::capture`.
        Command::new(
            "project.switcher.next",
            "Switch project (most recently used)",
            PROJECT,
        )
        .when("multipleProjects"),
        Command::new(
            "project.switcher.prev",
            "Switch project (least recently used)",
            PROJECT,
        )
        .when("multipleProjects"),
        // The immediate walk along the header strip. Unbound by default — Ctrl+Tab belongs to
        // the switcher — and kept because the two answer different questions: this one matches
        // the tabs on screen, the switcher matches the order the user works in. A user who
        // prefers strip order binds these and the switcher stops being reachable by key,
        // which is the whole reason both are registered rather than one being deleted.
        Command::new("project.next", "Next project (header order)", PROJECT)
            .when("multipleProjects"),
        Command::new("project.prev", "Previous project (header order)", PROJECT)
            .when("multipleProjects"),
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
        /*
         * Restart and resume, and why there are two of them.
         *
         * `claude.restart` carried `.unavailable("needs a respawn path in the pane host; kill
         * alone is not a restart")` — accurate at the time: only `TerminalPane` knows a pane's
         * geometry and holds the terminal a new child must attach to, so a command layer that
         * could only kill would have been a *stop* command wearing the word restart. The pane
         * publishes the respawn now (`ui/src/panes/paneRestart.ts`), so both of these run.
         *
         * They are separate ids rather than one command with an argument because **ids are
         * API**: a user's `keymap.json` names them and they are never renamed, so splitting one
         * later would leave an existing binding meaning whichever half we happened to pick.
         * Fresh and resume are also genuinely different acts — one starts an empty conversation,
         * the other hands the old `SessionId` back to `claude --resume`, which keeps it, so the
         * pane's binding and the project's primary session do not move.
         *
         * Neither is bound by default; a restart is not a per-minute gesture, and the pane's own
         * bar is where a user with a dead terminal is actually looking.
         */
        Command::new("claude.restart", "Restart Claude session", CLAUDE).when("claudePaneFocused"),
        // Whether a transcript still exists is a fact about the disk — a file under
        // `~/.claude/projects` — so no clause can claim it. The handler asks `session_resumable`
        // and says so when the answer is no, which is the rule this table's header states about
        // preconditions the webview cannot evaluate.
        Command::new(
            "claude.resume",
            "Resume Claude session in this pane",
            CLAUDE,
        )
        .when("claudePaneFocused"),
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
        /*
         * *Select Opened File* — IDEA's name for it, and the name a user types into the palette.
         *
         * The id is unchanged and must stay unchanged: ids are API, a `keymap.json` names them,
         * and a second id for the same act would list two palette rows that do the same thing.
         * The *title* is not API.
         *
         * # The clause, which was wrong in the direction that hides a working command
         *
         * It was `editorFocused`, which is strictly stronger than what the handler needs:
         * `editorFocused` is about the focused **pane**, and a file tab split with a shell pane
         * (`SplitIntent::Shell`) leaves the tab a file tab with a non-editor pane focused — so
         * the palette hid a row that would have worked. `fileTabActive` is the honest
         * precondition, and `shellWindow` is the other half: a detached-pane window renders no
         * Explorer at all, so revealing into it is revealing into nothing.
         *
         * The keyboard is a different matter — the gate never reads this field, see the module
         * header — and `ctrl+shift+e` is deliberately unconditional there, because "where is the
         * file I am editing" is asked most often from a terminal.
         */
        Command::new("file.reveal", "Select opened file", FILE)
            .when("shellWindow && fileTabActive")
            .keywords(&["reveal", "locate", "show", "sidebar", "explorer", "tree"]),
        /*
         * A scratch file: a buffer that is not part of the project, in a drawer cide owns.
         *
         * `shellWindow && projectOpen`. The project is what keys the drawer, and the shell
         * window is where the tab and the tree that the gesture ends in actually live — a
         * detached-pane window would create a file and open its tab in the window next door,
         * which is a gesture landing somewhere the user was not looking.
         *
         * The ellipsis is the house spelling for a command that opens something to answer
         * first, matching *File structure…* and *Go to line…*: this one raises the type picker,
         * because the extension is what decides the language and choosing it afterwards would
         * mean renaming a file to change its highlighting.
         */
        Command::new("scratch.new", "New scratch file…", FILE)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "scratch",
                "buffer",
                "temporary",
                "notes",
                "playground",
                "snippet",
            ]),
        /*
         * Git — and the clause here is `projectOpen`, not "a repository is open".
         *
         * It *was* `repoOpen`, and that is the whole of the bug this group is being rewritten
         * for. `repoOpen` was derived in `ui/src/keys/context.ts` from `ProjectRoot::repo`, a
         * DTO field `cide_core::workspace::project_root` set to `None` and **nothing anywhere
         * ever set to anything else** — git discovery lives in `cide-git`, which depends on
         * this crate, and writing a discovered id into `workspace.json` would go stale the
         * first time anyone ran `git init`. So the flag was false for every user of every
         * build, this whole group was filtered out of the palette, and `sidebar.git` went with
         * it. Nothing failed: a flag that is always false is indistinguishable from one that
         * happens to be false right now, which is exactly what the module header above warns
         * about and exactly what happened anyway.
         *
         * The fix is not a better supplier. There isn't one: whether a project contains a
         * repository is a fact about the disk that changes when a bash pane runs `git init`,
         * and any webview-side mirror of it is a cache with no invalidation. `projectOpen` is
         * the strongest claim this side of the wire can make honestly, and the handlers in
         * `ui/src/keys/dispatch.ts` ask `git_repos` — real discovery, on the Rust side — and
         * report when the answer is empty. Precondition in the clause *and* in the handler, as
         * the header says; the clause simply stops claiming to know something it cannot.
         *
         * The cost is a Push row offered in a project that turns out to have no repository,
         * which reports. The alternative cost was every git row hidden from everybody, for
         * ever, in silence. `ProjectRoot::repo` no longer exists, so the flag cannot come back
         * by accident.
         */
        // Opens the Git panel with the caret in its message box, rather than committing.
        //
        // It was `.unavailable("needs the Git panel's message and ticked paths, which are not
        // in a store")`, and that reasoning was right about the mechanism and wrong about the
        // command. A commit needs a message and a set of paths; both live in `useGitPanel`'s
        // React state, which does not exist while the sidebar is on Files or shut. Lifting
        // them into a store to let a palette row commit whatever happened to be ticked, with
        // whatever text happened to be in the box, would be a foot-gun in a build with no
        // revert surface — and a half-typed commit message is not a mirror of anything Rust
        // owns, so it does not belong in a store either.
        //
        // So it does what the two branch commands one screen down already do for the same
        // reason: it puts the user in front of the control that asks. The trailing "…" is this
        // table's mark for that.
        Command::new("git.commit", "Commit changes…", GIT).when("shellWindow && projectOpen"),
        Command::new("git.push", "Push to remote", GIT).when("projectOpen"),
        Command::new("git.refresh", "Refresh git status", GIT).when("projectOpen"),
        // Branches. The two that need a name or a choice open the branch popup
        // (`ui/src/chrome/BranchSelector.tsx`) rather than acting blind; the two network ones
        // take no input at all and run.
        //
        // The popup is an overlay, not a child of the status bar widget, precisely so these
        // rows work in a window whose status bar has not mounted the widget — a palette entry
        // that depends on a control being on screen is a palette entry that sometimes does
        // nothing.
        Command::new("git.branch.switch", "Switch branch…", GIT).when("projectOpen"),
        Command::new("git.branch.new", "New branch…", GIT)
            .when("projectOpen")
            // "create branch" is what a user types when they want this, and none of the five
            // scoring tiers reaches "New branch…" from it — the needle is longer than the
            // title, so even the subsequence tier misses. Keywords are the fix; see `search`.
            .keywords(&["create", "make", "checkout"]),
        Command::new("git.fetch", "Fetch from remote", GIT)
            .when("projectOpen")
            .keywords(&["download", "remote"]),
        // Fast-forward only, and the title says so: cide has no conflict-resolution surface,
        // so a pull that had to merge would leave a working tree nothing in the app can
        // finish. A divergence is reported with both counts. See `cide_git::branch::pull`.
        Command::new("git.pull", "Pull (fast-forward only)", GIT)
            .when("projectOpen")
            .keywords(&["update", "merge", "ff"]),
        Command::new("git.stageSelected", "Stage selected changes", GIT)
            .when("projectOpen")
            // Still unavailable, and unlike `git.commit` this one stays that way. "Selected"
            // names the tick state of the Git panel's tree, and there is no non-blind reading
            // of it from anywhere else — a row that merely revealed the panel would be lying
            // about what its own title says it does. The reason names the surface that works
            // instead, because "not in a store" told the user nothing they could act on.
            .unavailable(
                "needs the Git panel's ticked rows — \"selected\" is that tree's own state, and \
                 nothing outside the panel can read it; stage from the tree or its context menu",
            ),
        // Navigate — within the file the caret is in. (M12)
        //
        // `editorFocused` and nothing weaker throughout. `editorOpen` would offer a member walk
        // in a window whose focus is in a terminal, and the walk would move a caret the user
        // cannot see.
        Command::new("structure.file", "File structure…", NAVIGATE)
            .when("editorFocused")
            .keywords(&["outline", "members", "symbols", "popup"]),
        // Go to line. The `…` says it opens a box, matching "File structure…" above.
        //
        // `navigate.line` rather than `goto.line`: the group already spells this family
        // `navigate.*`, and ids are API — a second prefix for the same idea is one more thing a
        // user's `keymap.json` has to guess at.
        Command::new("navigate.line", "Go to line…", NAVIGATE)
            .when("editorFocused")
            // "goto" is what a user types, and none of the five scoring tiers reaches "Go to
            // line…" from it — the space is in the title and not in the needle.
            .keywords(&["goto", "jump", "line", "number", "column"]),
        Command::new("navigate.definition", "Go to definition", NAVIGATE)
            .when("editorFocused")
            .keywords(&["declaration", "jump", "resolve", "symbol", "source"]),
        /*
         * Find usages — IDEA's Alt+F7. (M14)
         *
         * A command of its own rather than a mode of the one above, and the separation is
         * load-bearing rather than tidy. **Ctrl+click** is the combined gesture: it asks the server
         * where the thing under the pointer is declared and, if the answer is "right here", lists
         * usages instead of jumping. That discriminator is a heuristic — `fn fmt` inside
         * `impl Display for Foo` resolves to the *trait's* `fn fmt`, so Ctrl+click on it jumps
         * rather than listing — and a heuristic needs an escape hatch that skips it entirely.
         * This id is that hatch, and it is why guessing wrong costs one keystroke rather than
         * leaving the user with no way to ask the question they meant.
         *
         * It also works from a *reference*, unlike the click's declaration branch: LSP answers
         * references from any occurrence, so Alt+F7 on a call site lists the call's siblings.
         *
         * `editorFocused` and nothing weaker, like every other member of this group.
         */
        Command::new("navigate.usages", "Find usages", NAVIGATE)
            .when("editorFocused")
            // "references" is what the protocol calls it, "callers" and "uses" are what a person
            // types, and none of the five title/id scoring tiers reaches "Find usages" from any
            // of them.
            .keywords(&["references", "callers", "uses", "who", "used", "search"]),
        // Back / Forward — the mouse's thumb buttons, and the only two commands in this group
        // that are *not* `editorFocused`.
        //
        // `projectOpen` and nothing narrower, deliberately. The thumb button is pressed wherever
        // the pointer happens to be, which in a terminal-centric app is most often over a
        // terminal pane — and a Back that only worked while a buffer had focus would be a Back
        // that did nothing most of the times it was pressed. The precondition that *is* real —
        // "there is somewhere to go back to" — is re-checked in the handler, per the rule stated
        // at the head of this file: a `when` gates the palette and never the keyboard.
        Command::new("navigate.back", "Back", NAVIGATE)
            .when("projectOpen")
            .keywords(&["previous", "jump", "history", "return", "backwards"]),
        Command::new("navigate.forward", "Forward", NAVIGATE)
            .when("projectOpen")
            .keywords(&["next", "jump", "history", "forwards"]),
        Command::new("navigate.nextMember", "Next member declaration", NAVIGATE)
            .when("editorFocused")
            .keywords(&["method", "function", "down"]),
        Command::new(
            "navigate.prevMember",
            "Previous member declaration",
            NAVIGATE,
        )
        .when("editorFocused")
        .keywords(&["method", "function", "up"]),
        // View.
        Command::new("picker.files", "Go to file", VIEW).when("projectOpen"),
        // `picker.symbols` is the id `crates/cide-core/src/keymap.rs`'s own fixture already
        // reached for before this existed. Ids are API — using the name the codebase had already
        // chosen costs nothing and means one fewer spelling in the world.
        Command::new("picker.symbols", "Go to symbol in project", VIEW)
            .when("projectOpen")
            // None of these reaches the title through the five title/id tiers, and every one of
            // them is what somebody would actually type.
            .keywords(&[
                "member",
                "function",
                "method",
                "type",
                "definition",
                "declaration",
            ]),
        /*
         * The External Libraries group, reached without a mouse.
         *
         * The group is a row in the file tree and the tree is virtualized, so on a repository
         * with any depth to it the header can be thousands of rows below the viewport — the one
         * place in this app where a feature is *drawn* and still effectively unreachable. This
         * scrolls to it and opens it, which is also what starts the resolution.
         *
         * `shellWindow && projectOpen`: the sidebar exists only in the shell window, and a
         * project is what has dependencies. The handler still checks — the clause gates the
         * palette, never the keyboard — and reports when the project has no manifest and so no
         * group, which is the honest answer rather than a scroll to nowhere.
         *
         * No default binding. Every free chord in this app is free because somebody wanted it,
         * and a browser for a group most users open once a session does not outrank them; the
         * palette and `keymap.json` are both one step away.
         */
        Command::new("view.externalLibraries", "Show External Libraries", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "dependencies",
                "crates",
                "modules",
                "cargo",
                "go",
                "packages",
                "vendor",
            ]),
        /*
         * And the same route to the other group, for the same reason.
         *
         * *Scratches* sits below *External Libraries* at the very bottom of a virtualized tree,
         * so on any repository with depth to it the header is thousands of rows past the
         * viewport. `scratch.new` reveals the group as a side effect of creating a file; this is
         * how somebody who wants to *find* the scratches they already have gets there without
         * scrolling, and it is the only route that does not first create something.
         *
         * `projectOpen` because the drawer is keyed by the project's primary root; the handler
         * re-checks, because the clause gates the palette and never the keyboard.
         */
        Command::new("view.scratches", "Show Scratches", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["scratch", "buffer", "temporary", "notes", "playground"]),
        Command::new("palette.commands", "Show all commands", VIEW),
        Command::new("theme.toggle", "Toggle light/dark theme", VIEW),
        // Settings opens as a *tab inside a project*, so with no project open there is
        // nowhere to put it.
        Command::new("settings.open", "Open settings", VIEW).when("projectOpen"),
        Command::new("settings.keymap", "Open keyboard shortcuts", VIEW).when("projectOpen"),
        // The rail and its sidebar exist in the shell window only; a detached pane has none.
        Command::new("sidebar.files", "Show files sidebar", VIEW).when("shellWindow"),
        /*
         * F4 — hide the left panel, or bring back the last one that was open.
         *
         * The *hidden* state has existed since M3 and shipped reachable by mouse only: clicking
         * the lit rail button already sets the view to `null`, `ActivityRail::active` has always
         * been nullable, all four panel branches are `view === '…' &&` guards and the splitter
         * is withheld with them. There was no command, no binding and no palette row — the
         * sixteenth instance of this project's signature defect, and the only new *behaviour*
         * here is remembering which panel to restore.
         *
         * `shellWindow` alone. Hiding the panel is meaningful with no project open (the tree is
         * empty and the workspace still wants the width), which is why this matches
         * `sidebar.files` above rather than the `&& projectOpen` its neighbours carry.
         *
         * The settings view counts as *closed*, because it is: ⚙ opens a workspace tab and has
         * no panel, so F4 with it lit opens the last real panel and un-lights the gear. Treating
         * it as open would make the key look broken exactly once per session.
         */
        Command::new("sidebar.toggle", "Toggle the left panel", VIEW)
            .when("shellWindow")
            .keywords(&["hide", "show", "close", "panel", "explorer", "collapse"]),
        // `repoOpen` used to be half of this clause, which is why the palette could not even
        // open the Git sidebar — the row that would have let a user *look* at the repository
        // was hidden by the same permanently-false flag as the commands that act on it.
        Command::new("sidebar.git", "Show git sidebar", VIEW).when("shellWindow && projectOpen"),
        // The search panel, which had no command at all: it was reachable from the ⌕ button in
        // the activity rail and from nothing else — no palette row, no binding, and no way to
        // put the caret in its box without a mouse. `projectOpen` rather than bare
        // `shellWindow` because `SearchStore.setQuery` returns without asking anything when
        // there is no project, so the panel would open onto a box that cannot search.
        Command::new("sidebar.search", "Show search sidebar", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["find", "grep", "in files"]),
        // The ⚑ rail button has emitted `'problems'` since M3 and `App.tsx` has handled it since
        // M11, but there was no command and no binding — so the view was mouse-only. Exactly the
        // gap `sidebar.search` had, one view later, and closing it is two lines.
        Command::new("sidebar.problems", "Show problems sidebar", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["diagnostics", "errors", "warnings", "inspections"]),
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
        // `groups()` derives its order from first appearance in `build()`, so this pins the
        // *table's* order as much as the list — and "Navigate" sits between Git and View because
        // that is where the M12 block was inserted, not because anything sorted it.
        assert_eq!(
            groups(),
            vec![
                "Window", "Project", "Claude", "Terminal", "File", "Git", "Navigate", "View",
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
    fn the_switchers_own_their_chords_and_the_header_walk_stays_registered_and_unbound() {
        // Two families that answer different questions, and the way this goes wrong is that
        // one of them quietly becomes unreachable. So both halves are pinned here.
        for id in [
            "project.switcher.next",
            "project.switcher.prev",
            "project.next",
            "project.prev",
        ] {
            let command = by_id(id).unwrap_or_else(|| panic!("{id} is registered"));
            assert_eq!(command.when.as_deref(), Some("multipleProjects"), "{id}");
            assert_eq!(command.unavailable, None, "{id} must be runnable");
        }
        for id in ["tab.switcher.next", "tab.switcher.prev"] {
            let command = by_id(id).unwrap_or_else(|| panic!("{id} is registered"));
            assert_eq!(
                command.when.as_deref(),
                Some("shellWindow && multipleTabs"),
                "{id}"
            );
            assert_eq!(command.unavailable, None, "{id} must be runnable");
        }

        /*
         * The *whole* tab-keyed set, exactly, and that exactness is the point.
         *
         * The mistake this closes is adding `ctrl+backquote → project.switcher.next` while
         * forgetting to delete the `ctrl+tab` line: two keys naming one command is not a
         * conflict, `the_default_keymap_has_no_conflicts` stays green, and nothing else in this
         * workspace notices that Ctrl+Tab is still the project switcher. Pinning the list rather
         * than asserting a membership is what makes the deletion verifiable.
         */
        let on_tab: Vec<String> = crate::keymap::defaults()
            .iter()
            .filter(|b| b.key.contains("tab"))
            .map(|b| b.command.clone())
            .collect();
        assert_eq!(
            on_tab,
            ["tab.switcher.next", "tab.switcher.prev"],
            "Ctrl+Tab is the tab switcher and nothing else is bound on that key"
        );

        // …and the key the project switcher moved to, from the other side. `backquote` is what
        // the browser reports for the key left of `1`; the table writes the literal, and
        // `normalize_key` renames nothing, so this looks for the literal.
        let on_backquote: Vec<String> = crate::keymap::defaults()
            .iter()
            .filter(|b| b.key.ends_with('`'))
            .map(|b| b.command.clone())
            .collect();
        assert_eq!(
            on_backquote,
            ["terminal.splitBelow", "project.switcher.next"],
            "Ctrl+` switches project, one Shift away from splitting a terminal below"
        );
    }

    /// The Git group asks for an open **project**, and nothing anywhere asks for `repoOpen`.
    ///
    /// This test used to assert the opposite — `Some("repoOpen")` on every git command — and
    /// it passed for as long as the bug existed, because it was pinning the clause rather than
    /// anything about the clause being satisfiable. `repoOpen` was supplied by the frontend
    /// from `ProjectRoot::repo`, which Rust's one constructor set to `None` and no code ever
    /// set to anything else, so the flag was false for every user of every build and this
    /// entire group — plus *Show git sidebar* — was filtered out of the command palette in
    /// silence. The full account is above the group in [`build`].
    ///
    /// Three assertions, failing for three different reasons:
    ///
    /// 1. the decision — `projectOpen` is the strongest claim the webview can make honestly,
    ///    and whether the project *holds* a repository is asked of the disk by the handler
    ///    (`git_repos`) at the moment the command runs;
    /// 2. the flag is gone from the vocabulary, so no future clause can name it and quietly
    ///    become unreachable again;
    /// 3. and no clause names it, which (2) already implies through
    ///    `every_when_clause_uses_the_documented_vocabulary` but which is stated here because
    ///    this is where a reader comes looking for the rule.
    #[test]
    fn git_commands_ask_for_a_project_and_nothing_asks_for_the_dead_repo_flag() {
        for command in registry().iter().filter(|c| c.id.starts_with("git.")) {
            let when = command.when.as_deref().unwrap_or("<none>");
            assert!(
                when.contains("projectOpen"),
                "{} is gated on `{when}`; the Git group asks for a project and lets the \
                 handler ask the disk about repositories",
                command.id
            );
        }

        assert!(
            !CONTEXT_FLAGS.contains(&"repoOpen"),
            "`repoOpen` was false for every user of every build; nothing may re-add it \
             without a supplier that reads the disk"
        );

        for command in registry() {
            let when = command.when.as_deref().unwrap_or("");
            assert!(
                !when.contains("repoOpen"),
                "{} names repoOpen, which nothing supplies",
                command.id
            );
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

    /// The phrase from the report, against the shipped table.
    ///
    /// `create new branch` matched nothing before [`TIER_KEYWORD`] and [`TIER_ALL_WORDS`]
    /// existed: the five tiers above them all match the needle *whole* against one string, and
    /// no title contains the word `create` — so the needle is not a prefix of `New branch…`,
    /// not one of its words, not a substring, not in the id, and (being longer than the title)
    /// not even a subsequence of it. The palette said *No matching commands* about a command
    /// that was right there, which a user cannot tell apart from the command not existing.
    ///
    /// Asserted against `registry()` rather than a fixture on purpose: the claim is about the
    /// keywords this build actually ships, and a fixture would let them be dropped from the
    /// table with this test still green.
    #[test]
    fn the_words_a_user_types_for_a_new_branch_find_the_new_branch_command() {
        assert_eq!(ids(&search("create new branch")), ["git.branch.new"]);
        // Order-independent: the tier is a set of words, not a sequence.
        assert_eq!(ids(&search("branch create")), ["git.branch.new"]);
        // And on its own, through the keyword tier alone. First rather than only: `create` is
        // also a subsequence of a couple of unrelated titles, which is the bottom tier doing
        // exactly its job — the assertion is that a declared keyword outranks an accident.
        assert_eq!(ids(&search("create"))[0], "git.branch.new");
    }

    /// A keyword is matched at a word boundary, and only for the command that declares it.
    ///
    /// The tier is already the vaguest hit the palette offers — a guess about vocabulary
    /// rather than a match on anything the user can see — so a substring match inside a
    /// keyword would turn it into noise. `create` must not answer to `eat`.
    #[test]
    fn a_keyword_matches_a_word_and_not_a_fragment_of_one() {
        assert!(ids(&search("update")).contains(&"git.pull"));
        assert!(
            !ids(&search("eat")).contains(&"git.branch.new"),
            "`eat` is inside `create` and must not reach it"
        );
        // The word-boundary rule is what makes a multi-word keyword usable: `in files` is one
        // keyword on `sidebar.search`, and `files` has to find it.
        assert!(ids(&search("in files")).contains(&"sidebar.search"));
    }

    /// Every word of a multi-word query has to land somewhere, or the row is not offered.
    ///
    /// A tier that matched "most of the words" would put the whole table under any query with
    /// one common word in it, which is the opposite of what the palette is for.
    #[test]
    fn a_multi_word_query_needs_all_of_its_words() {
        assert!(ids(&search("switch branch")).contains(&"git.branch.switch"));
        assert!(
            !ids(&search("switch zzzz branch")).contains(&"git.branch.switch"),
            "a word that matches nothing rules the row out"
        );
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

    #[test]
    fn back_and_forward_are_not_scoped_to_an_editor() {
        for id in ["navigate.back", "navigate.forward"] {
            let entry = by_id(id).unwrap_or_else(|| panic!("{id} is registered"));
            assert_eq!(
                entry.when.as_deref(),
                Some("projectOpen"),
                "{id} must not require a focused editor: the thumb button is pressed wherever \
                 the pointer is, which in this app is most often over a terminal, so \
                 `editorFocused` would make Back do nothing most of the times it is pressed"
            );
            assert!(
                entry.unavailable.is_none(),
                "{id} is dispatched, so it must not carry an `unavailable` reason"
            );
        }
        // Both are reachable from the palette by the word a user would type for them.
        assert!(search("back").iter().any(|c| c.id == "navigate.back"));
        assert!(search("forward").iter().any(|c| c.id == "navigate.forward"));
    }
}
