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
    // Narrower than `fileTabActive`: the tab this window shows is about a file *and* that file
    // is one Compose would read. (M48)
    //
    // Derivable from the mirror as Rust actually fills it — `TabKind::File` carries the path and
    // `cide_docker::compose::is_compose_file` decides by name — which is the rule the `repoOpen`
    // disaster left behind, and the reason this is a flag rather than a check inside the handler.
    // Six Compose rows offered on every `.rs` file, each of them inert, is precisely the
    // listed-but-silently-dead state this module exists to make unrepresentable.
    "composeFileActive",
    // The focused pane is a file tab's editor pane and the file is an Excalidraw drawing. (M63)
    //
    // Pane-level, like `editorFocused`, and not tab-level like `composeFileActive`: it exists to
    // *narrow* `editorFocused`, which the drawing pane would otherwise satisfy — it is a
    // `PaneKind::Editor` pane in a `TabKind::File` tab — so that every binding carrying
    // `editorFocused` stopped swallowing the chord it shares with the canvas. `ctrl+g` is *Go
    // to line* to the editor and *Group* to Excalidraw; `ctrl+equal`/`ctrl+minus` are fold and
    // unfold to one and zoom to the other; `alt+up`/`alt+down` walk members in a buffer that
    // has none. Each of those reached `dispatch.ts`, found no caret, logged a line, and the
    // canvas never saw the key. `keys/context.ts` derives it by name from the tab's path — the
    // same rule `composeFileActive` follows, and derivable from the mirror as Rust fills it —
    // and defines `editorFocused` as "an editor pane that is *not* a drawing".
    "drawingFocused",
    "claudeTarget",
    // There is deliberately no `repoOpen`. See the note above the Git group in [`build`]: the
    // webview cannot answer "does this project contain a git repository" without asking the
    // disk, and the flag it used to answer it with was permanently false.
    "shellWindow",
    // Host flags: transient chrome that only the React tree knows about.
    "overlayOpen",
    // Narrower than `overlayOpen`, which is true for any of ten overlays. A binding scoped to
    // this one reaches the keyboard only while Ctrl+P's list has it, which is what lets ⌥L be
    // free everywhere else — including in every terminal pane in every window, where the gate
    // is a *capture* listener and would otherwise swallow it.
    "filePickerOpen",
    "contextMenuOpen",
    "sidebarFiles",
    "sidebarGit",
    // A diff surface is mounted and in front — the git diff tab, the log tool window's revision
    // diff, or the **conflict resolver**, which is a diff of three documents and walks its
    // blocks with the same command.
    //
    // A host flag rather than a derived one, and the reason is the host/derived split above
    // rather than convenience: the answer comes from a claim stack in the webview
    // (`ui/src/panes/changeNav.ts`), which is transient chrome of exactly the kind only React
    // knows about. Every derived spelling was tried and is wrong in **both** directions. A tab
    // kind misses the tool window's revision diff, which is not a tab at all; and it is *true*
    // for a Claude `openDiff`, which is also `TabKind::Diff` and whose chunk walk is not wired
    // to this command. `fileTabActive` is not reusable either — `focusedTabPath` answers for a
    // diff tab only when its `new_path` is absolute, and a git-origin one is repo-relative.
    //
    // It cannot degenerate into the constant `repoOpen` became: the value is the length of a
    // stack that mounting, tab activation and a pointer press all move.
    "diffFocused",
    "gitlabReviewActive",
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
/// The group every contributed command is listed under. (M22)
///
/// One group and not one per extension, which was the other option and loses on the palette's own
/// terms: the group is a heading in a flat, fuzzy-matched list, and eleven headings with one row
/// each is a list with eleven headings. The extension's identity is in the id, which the palette
/// matches on, and in the row's title, which its author wrote.
pub const EXTENSIONS: &str = "Extensions";

/// A palette row for a command an extension contributes.
///
/// Not in [`registry`] and it cannot be: that is a `OnceLock<Vec<Command>>` handed out as
/// `&'static [Command]`, and the extension set changes while the app is running. `Bootstrap`
/// concatenates the two, which works because `Command::id` has always been a plain `String` rather
/// than an enum — a decision made long before there was anything to contribute, and the one that
/// made this cost a function instead of a redesign.
///
/// **Ids are API.** A user's `keymap.json` may name one of these, so an extension renaming a
/// command id orphans every binding pointing at it — the same rule the builtin ids follow, and the
/// reason `cide_ext::manifest` refuses an id it cannot make a stable prefix out of.
///
/// No `when` clause. A `when` may only name a flag in [`CONTEXT_FLAGS`], and a manifest naming one
/// would be naming an internal vocabulary it cannot see the definition of; a flag nobody sets is
/// false for ever and the command silently vanishes from the palette. Availability for a
/// contributed command is instead a fact about its extension — whether the worker is running —
/// which the frontend answers at dispatch.
#[must_use]
pub fn extension_command(id: &str, title: &str, keywords: &[String]) -> Command {
    let mut command = Command::new(id, title, EXTENSIONS);
    command.keywords = keywords.to_vec();
    command
}

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
/*
 * **There is no title-*substring* tier, and 4 is deliberately left empty.** (M48)
 *
 * There was one, between [`TIER_TITLE_WORD`] and [`TIER_ID`], and it could only ever fire for a
 * needle matching strictly *inside* a word — a word-boundary match has already returned one tier
 * above. That is an accident of spelling rather than a claim on the word, and the day
 * `Compose Recreate` was added is the day it cost something: `create` matched inside `Recreate`
 * and displaced `New branch…`, whose claim on that word is a *declared keyword*. Nobody typing
 * `create` into this palette means "re-create a Docker stack".
 *
 * The same trap was dodged once before by renaming a command — `git.tag.create` became
 * `git.tag.new` so it would not take `create` on [`TIER_ID`], and that note still stands beside
 * the test. Renaming cannot work here: `recreate` is Compose's own verb and the title has to say
 * it. So the tier is gone instead, which is the fix that generalises.
 *
 * Nothing became unfindable. A needle inside a word is still a subsequence of the title, so it
 * lands on [`TIER_SUBSEQUENCE`] and the row is still offered — one rank lower, under every
 * command that matched a word.
 */
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
    // A **word** of the id, not a substring of one — `word_prefix` and not `find`, the same rule
    // the keyword tier below already follows and for a sharper version of the same reason.
    //
    // This was `find` until M48, and the day `docker.compose.recreate` was added is the day it
    // cost something: `recreate` *contains* `create`, so a query of `create` scored on this tier
    // — which sits above keywords — and *Compose Recreate* displaced *New branch…*, whose claim
    // on that word is a declared keyword. Nobody typing `create` into this palette means
    // "recreate a Docker stack".
    //
    // The test above `git.tag.new` records that this trap was dodged once before by *renaming* a
    // command (`git.tag.create` → `git.tag.new`). That worked because cide owned the word; it
    // does not work here, because `recreate` is Compose's own verb and the id has to say it. So
    // the tier is narrowed instead, which is the fix that generalises: an id segment is a word,
    // and matching inside one was never what this tier meant.
    if let Some(offset) = word_prefix(&command.id.to_lowercase(), needle) {
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
/// Subagents and the project's task tracker. (M18)
///
/// Its own group rather than rows under [`CLAUDE`], which is about the pane in front of you —
/// "Pause subagents" sitting beside "Fork session into new pane" would read as two ways of
/// doing one thing.
const AGENTS: &str = "Agents";

/// Docker, and for now only the half that acts on a file. (M48)
///
/// Its own group rather than rows under [`FILE`]: *Compose Up* beside *Save All* reads as a thing
/// you do to the buffer. The container half of Docker is a panel and has no palette rows at all —
/// it acts on a row the user has selected there, which is not something a palette can name.
const DOCKER: &str = "Docker";

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
        // The row, not the tab: it evens out the tiles beside this pane and leaves every
        // other chain — the rows above and below it included — bit for bit as it was.
        Command::new("pane.evenRow", "Even out the panes in this row", WINDOW).when("paneFocused"),
        /*
         * Move the pane itself, rather than the focus. (M31)
         *
         * The user's words: *"hotkeys to move panel between rows/columns"*, beside a grab
         * handle in the pane's corner that does the same with the mouse.
         *
         * `terminalFocused`, not `paneFocused`, and the clause is doing real work. The handle
         * is drawn on claude and shell panes only, and one flag keeps the chord and the button
         * describing the same gesture — a palette row offering to move an editor pane that has
         * nothing to grab would be the listed-but-inert state this module exists to prevent.
         * The flag is also the honest one for the job: it is false for a diff pane, which an
         * earlier `kind != 'editor'` spelling counted as a terminal.
         *
         * The direction names the neighbour the pane moves *past*, and it is resolved by the
         * same `layout::navigate` walk `pane.navigate.*` uses — so these four and those four
         * can never disagree about which pane is up. At the edge of the tree the walk answers
         * nothing and the command is a no-op, deliberately: a move that wrapped would cycle a
         * layout for ever with no fixed point, which is a worse bargain than a focus that wraps.
         */
        Command::new("pane.move.left", "Move pane left", WINDOW).when("terminalFocused"),
        Command::new("pane.move.right", "Move pane right", WINDOW).when("terminalFocused"),
        Command::new("pane.move.up", "Move pane up", WINDOW).when("terminalFocused"),
        Command::new("pane.move.down", "Move pane down", WINDOW).when("terminalFocused"),
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
        // Project. Opening one, then two families for moving between the ones already open —
        // and the split between those two is the point rather than duplication.
        //
        // Opening a project had no command at all: the only two routes were the header's `+`
        // and the `▾` beside it, so the gesture was mouse-only and — because a `keymap.json`
        // names command ids — unbindable. Reported as "Missing 'Open project' command in
        // command palette".
        //
        // **No `when` clause**, and it is the one command here for which that is a claim rather
        // than an omission. `settings.open` and `help.about` carry `projectOpen` because they
        // need somewhere to appear — `App.tsx` mounts `OverlayHost` in the shell branch and only
        // with a project, so the palette itself cannot be raised without one either, and this
        // row is in practice only ever *picked* by someone who already has a project. A key
        // bound to it is the other half, and that one runs with nothing open at all: it raises
        // an OS dialog rather than anything of cide's, so it is the state the command is most
        // wanted in and a clause would be exactly wrong there.
        //
        // The ellipsis is this table's convention for a row that raises something rather than
        // acting — `git.commit`, `git.branch.switch`, `scratch.new` — and here what it raises
        // is the OS folder picker. `dispatch.ts` runs it through the same `browseForProject`
        // the header's `+` does, which goes to `project_pick` and never to
        // `@tauri-apps/plugin-dialog`: that dialog cannot be parented on Linux, so a second
        // road spelled here would reintroduce the picker-behind-the-window bug on the surface
        // least likely to be retested.
        Command::new("project.open", "Open project…", PROJECT).keywords(&[
            "folder",
            "directory",
            "add",
            "browse",
            "load",
        ]),
        // (M97) Creating one: the New project wizard — kind, location, OpenSpec, subagents, a
        // brief for the console. No `when` clause for `project.open`'s reason above: a key bound
        // to it runs in the empty shell, which is where a first project is made, and the wizard
        // is mounted beside `PushDialog` in `App.tsx` rather than in `OverlayHost` so that it can
        // be raised there.
        //
        // Not `create`, though it is the word: `git.branch.new` declares it, a tie in the keyword
        // tier falls to registry order, and this row sits above that one — so `create` would have
        // started answering with a project wizard, which the two tests pinning `create` to *New
        // branch…* exist to prevent. `new` is in the title and reaches it a tier higher anyway.
        Command::new("project.new", "New project…", PROJECT).keywords(&[
            "scaffold",
            "init",
            "wizard",
            "openspec",
            "milestones",
        ]),
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
        /*
         * The chord spellings of the two mouse gestures beside it, asked for by name: "CTRL+(
         * - new claude panel, CTRL+) - new claude row" (and the bash pair under Terminal
         * below). `claude.split.right` is the pane title bar's `⊞ claude pane` — a tile beside
         * the focused pane, `PaneTitleBar.tsx`'s exact call — and `claude.addRow` is the
         * header's `⊞ claude row`, the full-width row `RowControls.tsx` adds. Both create the
         * session they split to, so they carry `claude.split.newSession`'s clause and not
         * `claudePaneFocused`, for the reason its comment above walks through. "panel" is the
         * user's own word for a tile, so it is a keyword rather than lost.
         */
        Command::new(
            "claude.split.right",
            "Split right: new Claude session",
            CLAUDE,
        )
        .when("paneFocused")
        .keywords(&["panel", "tile", "beside"]),
        Command::new("claude.addRow", "Add row: new Claude session", CLAUDE)
            .when("paneFocused")
            .keywords(&["panel", "full-width"]),
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
        /*
         * Agents. (M18)
         *
         * Three rows. One more is still waiting on something that does not exist yet, so it is
         * not registered: `task.new` needs the tracker store and the `.cide/tasks.json` write
         * path. Listing it before its handler exists is precisely the state this table's header
         * spends a section on — a palette row that is advertised, chosen and silently does
         * nothing. `check-commands.mjs` makes that unrepresentable: an id here with no `case` in
         * `ui/src/keys/dispatch.ts` and no `unavailable` reason fails the build, and marking it
         * `.unavailable("…")` in the meantime would be worse than absence, because a greyed row
         * promising a feature is a promise the palette keeps showing until somebody removes it.
         *
         * `agents.pause` and `agents.resume` were on that waiting list until this slice and are
         * now the **project scope** of `agents_pause` / `agents_resume` — the nullable-`run`
         * commands, called with `null`: the queue is shut and every live run is frozen.
         *
         * **`agents.resume` is not a convenience, it is the way out.** The project-scope pause
         * freezes the project's own console session along with the runs, so the pane the user
         * would reach for is the pane that is stopped. A resume has to be reachable from
         * somewhere that is not it, and the palette — which is chrome, not a pane — is by
         * construction somewhere else. The Agents panel header is meant to carry the same pair;
         * until `AgentsPanelViewProps` grows the props for it (`AgentsPanelHost`'s header names
         * them), this is the only control that can thaw a frozen console.
         *
         * **No new context flag, and that is the load-bearing part.** The obvious clause for
         * both is `subagentsEnabled`, and it must not exist: whether subagents are on is a fact
         * only the disk knows (`.cide/config.json` in the project), the workspace mirror cannot
         * derive it, and a flag the webview cannot supply is false for ever — which is the
         * `repoOpen` disaster this module's header spends fifty lines on, the one that hid every
         * git command from the palette while a test asserted the clause. So these rows ask for
         * `projectOpen`, the strongest claim the webview can make honestly, and the handler asks
         * `agents_config_get` over IPC and answers with a sentence the user can read.
         *
         * **No default binding either.** Every chord worth having is taken — `check:keymap`
         * would surface a real conflict rather than a hypothetical one — and all three are
         * palette- and mouse-reachable, which is the standing `claude.restart` already has.
         */
        Command::new("agents.pause", "Pause agents", AGENTS)
            .when("projectOpen")
            .keywords(&["freeze", "stop", "suspend", "sigstop", "subagents"]),
        Command::new("agents.resume", "Resume agents", AGENTS)
            .when("projectOpen")
            .keywords(&["thaw", "unfreeze", "continue", "sigcont", "subagents"]),
        Command::new("task.focusBoard", "Go to tasks", AGENTS).when("shellWindow && projectOpen"),
        /*
         * The pool-state card (M90): every model pool's entries with their load, which ones a
         * refusal has benched and for how long, who is on what, and why recent runs started
         * where they did — with Reset beside a benched entry and Test beside every one.
         *
         * Asked for as *"my default pool is always starting from the last element and I have no
         * visual info why"*: a run's row said `6 of 6` and nothing about the five before it.
         *
         * `shellWindow && projectOpen` for `help.about`'s reason: `OverlayHost` is mounted
         * there only, and a keymap binding toggling the store anywhere else would leave an
         * invisible overlay gating every `!overlayOpen` binding. The pools themselves are
         * global, so any open project will do.
         */
        Command::new("agents.poolState", "Model pools: show state", AGENTS)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "llm", "pool", "model", "failover", "penalty", "bench", "reset", "provider",
                "opencode",
            ]),
        // Terminal. Splitting one below is offered from any pane — it creates the terminal
        // it splits to — whereas clearing needs a terminal already focused.
        Command::new("terminal.splitBelow", "Split terminal below", TERMINAL).when("paneFocused"),
        // The bash half of the spawn-chord family — see `claude.split.right` above for the
        // shape. A tile beside the focused pane, and the header's `⊞ bash row`. "bash" and
        // "panel" are the words the ask used, so both are keywords.
        Command::new("terminal.splitRight", "Split terminal right", TERMINAL)
            .when("paneFocused")
            .keywords(&["bash", "panel", "tile", "beside"]),
        Command::new("terminal.addRow", "Add row: new shell", TERMINAL)
            .when("paneFocused")
            .keywords(&["bash", "panel", "full-width"]),
        Command::new("terminal.clear", "Clear terminal", TERMINAL).when("terminalFocused"),
        Command::new("terminal.paste", "Paste into terminal", TERMINAL).when("terminalFocused"),
        /*
         * Search this pane's transcript. Asked for as *"Need to add CTRL+F (search bar) for
         * claude and bash panels to be able to search text"*.
         *
         * # It ships with no default binding, and that is the decision, not an omission
         *
         * The chord is Ctrl+F and it is resolved in `ui/src/terminal/keys.ts`, from xterm's own
         * custom key handler, exactly where Ctrl+C and Ctrl+V are resolved and for the same
         * reason. A `("ctrl+f", "terminal.find", "terminalFocused")` line in
         * [`crate::keymap::defaults`] was the obvious alternative and it is the one this
         * codebase has already written down three separate times as wrong:
         *
         * * `keymap::the_terminal_clipboard_chords_are_not_bound_here` states it in full — the
         *   key gate's second entry point is a window **capture** listener, so a binding is
         *   consumed before the event reaches its target, and `terminalFocused` is derived from
         *   `tab.tree.focused` rather than from the caret. It stays true while the user is typing
         *   in a rename field, the commit message box or the Explorer's search input, so the
         *   binding would take Ctrl+F from all of them in any window whose focused pane happens
         *   to be a terminal — which, in an app with a pinned Claude console, is most windows
         *   most of the time.
         * * `ui/src/editor/EditorSurface.tsx` says it about this exact chord: binding `ctrl+f`
         *   here "fights a written rule", because CodeMirror's `Mod-f` opens the *editor's* find
         *   bar and reaches it only because this keymap stays silent about the key.
         * * `keymap::nothing_binds_the_find_bars_f_keys` keeps F3/Shift+F3 out for the third
         *   version of the same argument.
         *
         * So the command is here — a palette row, and one line of `keymap.json` away from ⌘F on
         * a Mac or from any chord a user prefers — while the *shipped* chord is focus-scoped in
         * the terminal, where it can only ever reach a terminal that has the keystroke. That is
         * the same shape `terminal.clear` and `terminal.paste` already have: registered, gated,
         * dispatched, and bound by nobody.
         *
         * `terminalFocused` because the bar belongs to a pane and there has to be one. There is
         * deliberately no *find next* / *find previous* command beside it: those live on Enter
         * and Shift+Enter inside the bar itself, and giving them ids would mean giving them
         * chords, and the only conventional ones are F3 and Shift+F3, which the rule above
         * forbids.
         */
        Command::new("terminal.find", "Find in terminal", TERMINAL)
            .when("terminalFocused")
            .keywords(&["search", "find", "grep", "scrollback", "transcript"]),
        // File.
        // `editorFocused || drawingFocused`: the drawing pane is not an editor for the purposes
        // of every other clause here (see `drawingFocused` in `CONTEXT_FLAGS`), and it saves.
        // The *binding* is unconditional either way — this clause is the palette's filter.
        Command::new("file.save", "Save file", FILE).when("editorFocused || drawingFocused"),
        Command::new("file.saveAll", "Save all files", FILE).when("editorOpen"),
        /*
         * Reformat code — Shift+Alt+F. (M26)
         *
         * [`FILE`] and deliberately not [`VIEW`]. The fold family's comment above `editor.fold`
         * defines this group boundary as *changes what is on screen and changes nothing about
         * the file*, and reformatting is the other side of that line: it edits the buffer, it
         * dirties the tab, and Ctrl+Z undoes it. Its neighbours here are the two commands that
         * also write the user's text.
         *
         * **It does not save.** The formatted text is dispatched into the buffer and the tab
         * goes dirty like any other edit. Formatting the file *on disk* instead would be the
         * write-under-a-dirty-buffer that `cide_core::document`'s
         * `a_stale_precondition_refuses_and_leaves_the_file_alone` calls the one that matters —
         * and its worked example is literally `cargo fmt`.
         *
         * Two roads behind one id, resolved in `cide_app::lsp`: a formatter configured in
         * Settings → Editor → Formatters wins, otherwise the language server's
         * `textDocument/formatting`. One id because the user's question is "reformat this", not
         * "reformat this using a particular mechanism" — and because a second id would be a
         * second thing to bind, list and explain for a distinction they cannot act on.
         *
         * `editorFocused` and nothing weaker, like every other command that needs a caret. The
         * keyword list is long on purpose: none of the five title/id scoring tiers reaches
         * "Reformat code" from `fmt`, `prettier` or `gofmt`, which is what a person types.
         */
        Command::new("editor.format", "Reformat code", FILE)
            .when("editorFocused")
            .keywords(&[
                "format", "fmt", "rustfmt", "gofmt", "prettier", "indent", "tidy", "beautify",
                "reindent",
            ]),
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
         * The Explorer header's *Expand all* / *Collapse all*. (M96)
         *
         * Commands rather than bare button handlers, on `file.reveal`'s argument: one code
         * path for the button, the palette row and any chord a `keymap.json` binds. The clause
         * is `shellWindow` alone, because only the shell window draws an Explorer and the tree
         * belongs to the project rather than to whatever tab is active.
         */
        Command::new("file.expandAll", "Expand all folders", FILE)
            .when("shellWindow")
            .keywords(&[
                "expand", "unfold", "open", "all", "explorer", "tree", "folders",
            ]),
        Command::new("file.collapseAll", "Collapse all folders", FILE)
            .when("shellWindow")
            .keywords(&[
                "collapse", "fold", "close", "all", "explorer", "tree", "folders",
            ]),
        /*
         * What a file *is*: OS stat, text facts and the git summary, in one card. (M70)
         *
         * **No `shellWindow`**, unlike `file.reveal` directly above, and the asymmetry is
         * deliberate — it is `git.blame`'s rule. That command's clause omits it because the
         * gutter is a property of the *buffer* and a detached-pane window can hold one; this one
         * omits it because the card is an **overlay**, any window can raise one, and a command
         * that works in a window has to be listable in it. The card's one window-dependent
         * control, *Open full history*, is simply not drawn where there is no tool window
         * (`FileProperties.tsx`'s `onOpenHistory` prop), rather than the whole command
         * disappearing from the palette of the window it works in.
         *
         * `fileTabActive` and not `editorFocused`, `file.reveal`'s distinction exactly: this is
         * about the tab rather than the focused pane, so a file tab split with a shell pane
         * still answers. Both context menus pass an explicit path and never rely on the clause.
         *
         * No default binding. `alt+enter` is the IDE convention for this and is already
         * `pane.maximize` (see `keymap::defaults`); displacing it to shave a right-click off a
         * command nobody runs hourly is the wrong trade, and `git.history.file` — the command
         * this one sits beside in every menu — ships with no chord for the same reason.
         */
        Command::new("file.properties", "File properties", FILE)
            .when("projectOpen && fileTabActive")
            .keywords(&[
                "info",
                "stat",
                "size",
                "permissions",
                "metadata",
                "details",
                "owner",
                "mode",
            ]),
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
         * The project's notes file: one pinned markdown file per project, opened as an ordinary
         * editor tab.
         *
         * The row is pinned to the **top** of the file tree rather than the bottom, so unlike
         * the two `view.*` rows below it is not unreachable-by-scrolling — but it is still worth
         * a command for the two reasons that are always worth one here: a gesture with no
         * command has no chord anyone can bind in `keymap.json`, and the row's double-click, the
         * palette row and any future binding must not become three code paths that behave in
         * three ways. `ui/src/App.tsx` routes the click through `runCommand` for exactly that,
         * the same way `Explorer`'s ⌖ button routes through `file.reveal`.
         *
         * `shellWindow` because the tab and the tree the gesture ends in live there — a
         * detached-pane window would create a file and open its tab in the window next door.
         * `projectOpen` because `roots[0]` is what keys the file. The handler re-checks both:
         * the clause gates the palette and never the keyboard.
         *
         * No default binding. Every free chord in this app is free because somebody wanted it,
         * and the palette and `keymap.json` are both one step away.
         *
         * No ellipsis: it opens a tab rather than asking a question first, which is the house
         * rule *New scratch file…* one screen up is on the other side of.
         */
        Command::new("file.projectNotes", "Open Project Notes", FILE)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "notes",
                "notepad",
                "todo",
                "scratchpad",
                "markdown",
                "journal",
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
        Command::new("gitlab.open", "Open GitLab merge request by URL…", GIT),
        Command::new("gitlab.nextFile", "Next MR file", GIT).when("gitlabReviewActive"),
        Command::new("gitlab.previousFile", "Previous MR file", GIT).when("gitlabReviewActive"),
        // Walk the threads and agent drafts of the MR file in front, in reading order. Stays in
        // the file and clamps at the ends (`ui/src/gitlab/commentNav.ts`): Alt+PgDn/PgUp above
        // already move between files.
        Command::new("gitlab.nextComment", "Next MR comment", GIT)
            .when("gitlabReviewActive")
            .keywords(&["discussion", "draft", "thread", "review"]),
        Command::new("gitlab.previousComment", "Previous MR comment", GIT)
            .when("gitlabReviewActive")
            .keywords(&["discussion", "draft", "thread", "review"]),
        Command::new("git.push", "Push to remote…", GIT).when("projectOpen"),
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
        /*
         * IDEA's name for it, and the same name the commit panel's toolbar button carries.
         *
         * It was "Pull (fast-forward only)" until M20, because cide had no conflict-resolution
         * surface and a pull that had to merge would have left a working tree nothing in the
         * app could finish. It has one now (`cide_git::conflict`, and the resolver behind the
         * commit panel's *Merge Conflicts* group), so this fetches and then integrates — by
         * fast-forward where it can, and otherwise by whatever `pull.rebase` says or the user
         * chooses in the dialog.
         *
         * **The id does not change and must not.** Ids are API: a user's `keymap.json` names
         * them, and `ctrl+t` is bound to this one by default.
         */
        Command::new("git.pull", "Update project", GIT)
            .when("projectOpen")
            .keywords(&["pull", "update", "merge", "rebase", "ff"]),
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
        /*
         * The git tool window — the bottom panel holding the commit log, per-file history and
         * the blame gutter's click-through — plus the one commit action the palette can name
         * on its own. (M18)
         *
         * Four rows, and deliberately only four. `git.reset`, `git.revert` and `git.cherryPick`
         * are **not** registered, and that is now a choice about the *palette* rather than about
         * the backend: all three are live `#[tauri::command]`s (`cmd/git.rs`'s commit-actions
         * block) over `cide-git` code with 33 differential tests behind it. Each needs a
         * **commit** the palette cannot name, and unlike `git.stageSelected` there is no honest
         * default to fall back on — resetting to HEAD is a no-op, and a bare *Revert* sitting in
         * the palette beside the Git panel's *Rollback* is a genuine ambiguity that IDEA has and
         * that confuses people. The option that lost was three `.unavailable(…)` rows on
         * `git.stageSelected`'s pattern; it loses because one live row that opens the log puts
         * all of them one right-click away, whereas three dead rows are three dead rows. Ids are
         * API and can be added later; a dead row cannot be taken back.
         *
         * `git.tag.new` **is** registered, and the asymmetry is the same argument reaching the
         * other answer rather than an exception to it. Tagging **HEAD** is a meaningful default
         * the palette can express: *mark where I am standing* is the common case, `TagRequest`
         * carries a revspec so `HEAD` needs no row to have been clicked, and the one thing the
         * gesture genuinely needs — a name — is what its dialog asks for. That is exactly the
         * shape `git.branch.new` already has one screen up, and what the trailing `…` in both
         * titles means: the row opens the control that asks rather than guessing.
         *
         * `shellWindow` on the first two because a detached-pane window renders no tool window at
         * all, so revealing into it is revealing into nothing — the same half of the clause
         * `file.reveal` carries for the Explorer. Not on `git.tag.new`, for `git.blame`'s
         * reason: its dialog is an overlay, any window can raise one, and a command that works in
         * a window has to be listable in it.
         */
        Command::new("git.log", "Show git log", GIT)
            .when("shellWindow && projectOpen")
            .keywords(&["history", "commits", "graph", "revisions"]),
        // `fileTabActive`, not `editorFocused`: a file tab split with a shell pane is still a
        // file tab, and this command is about the tab rather than about the focused pane. That
        // is the distinction `file.reveal`'s comment spells out, derived from the same
        // `focusedTabPath`.
        Command::new("git.history.file", "Show history for this file", GIT)
            .when("shellWindow && projectOpen && fileTabActive")
            .keywords(&["log", "commits", "revisions", "versions", "annotate"]),
        // No `shellWindow`, and that asymmetry is deliberate: the gutter is a property of the
        // *buffer*, a detached-pane window can hold one, and only the click-through to the log
        // needs the panel. That arm reports rather than the clause hiding the row — a command
        // that works in a window is a command that must be listable in it.
        Command::new("git.blame", "Annotate with git blame", GIT)
            .when("projectOpen && editorFocused")
            .keywords(&["blame", "annotate", "praise", "who", "gutter", "authors"]),
        // The tag dialog, defaulting to HEAD — see the block comment above for why this one of
        // the commit actions earns a palette row and the other three do not.
        //
        // No `mark` or `create` in the keywords. `create` is already `git.branch.new`'s, and a
        // second command answering to it would put two rows under a word that has one obvious
        // meaning in a git palette; the id carries the word anyway, which is `TIER_ID` — a tier
        // *above* keywords, so a user typing `create` gets this row regardless and the ranking
        // is the table's rather than a duplicated keyword's.
        Command::new("git.tag.new", "Tag commit…", GIT)
            .when("projectOpen")
            .keywords(&["tag", "release", "version", "label"]),
        /*
         * Compose, against the file in front of you. (M48)
         *
         * The third of the surfaces M48 gives these verbs — the file tree's context menu and the
         * editor's gutter are the other two — and the one that exists because the other two both
         * need a pointer to be somewhere. `shellWindow` because the run opens a pane and a
         * detached-pane window has nowhere to put one; `composeFileActive` because offering
         * *Compose Up* on a Rust file is the listed-and-inert row this module refuses.
         *
         * Here rather than after the Agents group, so `groups()` — whose order is first
         * appearance in this table — puts Docker beside File and Git. These act on the file that
         * is open, which is what those two groups are about; between *Go to tasks* and the
         * terminal commands they would read as being about the pane.
         *
         * No default bindings. Six chords for verbs reached from two better surfaces would spend
         * the keymap's remaining Ctrl+Shift space on something nobody asked for — and the two
         * that might earn one (`up`, `down`) are destructive enough that a mistyped chord takes a
         * stack down under somebody's hands. `keymap.json` is where a user disagrees.
         */
        Command::new("docker.compose.up", "Compose Up", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "start", "stack", "services"]),
        Command::new("docker.compose.down", "Compose Down", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "stop", "stack", "remove"]),
        Command::new("docker.compose.restart", "Compose Restart", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "stack"]),
        Command::new("docker.compose.recreate", "Compose Recreate", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "apply", "force", "stack"]),
        Command::new("docker.compose.build", "Compose Build", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "image", "stack"]),
        Command::new("docker.compose.pull", "Compose Pull", DOCKER)
            .when("shellWindow && composeFileActive")
            .keywords(&["docker", "compose", "image", "fetch", "stack"]),
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
        // IDEA's *Quick Documentation*. (M60) A separate id rather than a mode of
        // `navigate.definition`, for the reason the block below gives about implementation: it
        // is a different question with a different answer (a page, not a place), and a user
        // binds and greps for it under its own name. Go to definition falls back to it when
        // the server answers "no file" — Godot's built-ins, a JDK class — but the command is
        // how documentation is asked for on purpose.
        Command::new("navigate.documentation", "Quick documentation", NAVIGATE)
            .when("editorFocused")
            .keywords(&["docs", "help", "reference", "hover", "javadoc", "describe"]),
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
        /*
         * Go to implementation — IDEA's Ctrl+Alt+B. (M18)
         *
         * A **separate id**, not a mode of `navigate.definition`, and the separation is the whole
         * design rather than a filing decision.
         *
         * The report was Go-shaped: *"goto for golang goes to the interface declaration, but
         * should go to the implementation"*. gopls is answering correctly —
         * `textDocument/definition` on a call through an interface resolves to the interface's
         * method, because that is where the callee is declared. "Take me to the concrete one" is
         * `textDocument/implementation`, a different protocol request, and cide asked it nowhere.
         *
         * Teaching Ctrl+B to try implementation first and fall back **regresses Rust**:
         * rust-analyzer answers `implementation` on a struct name with its `impl` blocks and on a
         * trait with its implementors, so Ctrl+click on an ordinary type name would stop opening
         * the declaration. That is why this id exists at all.
         *
         * **Since the follow-up report, Ctrl+B does redirect — but only from one position.** Not
         * "in Go": from a definition that landed on a method inside `interface { … }`, which
         * `cide_lang::interface_method_at` decides by parsing the *target* file. A struct name, a
         * trait name and an interface *type* name all land outside that brace block and are
         * untouched, so neither regression above is reachable. This id keeps its own binding
         * because the redirect answers only the case where the declaration was useless, and
         * asking for implementors of a type you are standing on is a question a user still has.
         *
         * ≥2 answers is the *common* case (an interface with many implementors), so this reuses
         * the Find usages popup: 0 falls back to Go to definition so the command always does
         * something, 1 jumps, ≥2 shows the picker.
         *
         * `editorFocused` and nothing weaker, like every other member of this group.
         */
        Command::new("navigate.implementation", "Go to implementation", NAVIGATE)
            .when("editorFocused")
            // "implementors" and "concrete" are what a person types; "interface", "impl",
            // "override" and "subclass" are what they call the thing they are standing on. None
            // of the five title/id scoring tiers reaches this row from any of them.
            .keywords(&[
                "implementors",
                "impl",
                "concrete",
                "interface",
                "override",
                "subclass",
            ]),
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
        /*
         * Next / previous change, in a diff or the conflict resolver. (M25)
         *
         * Modelled on the member walk directly above, down to the clamp: both answer from a
         * model the webview already has, neither costs a round trip, and both stop at the ends
         * and say so rather than wrapping.
         *
         * `NAVIGATE` and not `GIT`, although every surface that answers is a git one. This is
         * where a reader looks for "next X", it belongs beside the walk it is modelled on, and
         * the group a command sits in is about where it is found rather than about which
         * subsystem implements it.
         *
         * The clause is `diffFocused` alone. The *binding* carries more — see
         * `keymap::defaults` — for the reason `picker.libraries` sets out at length: a `when`
         * here decides what the palette offers, and the palette should still offer this when
         * focus happens to be in a terminal split beside the diff.
         */
        Command::new("navigate.nextChange", "Next change", NAVIGATE)
            .when("diffFocused")
            .keywords(&[
                "diff",
                "difference",
                "hunk",
                "conflict",
                "merge",
                "block",
                "down",
            ]),
        Command::new("navigate.prevChange", "Previous change", NAVIGATE)
            .when("diffFocused")
            .keywords(&[
                "diff",
                "difference",
                "hunk",
                "conflict",
                "merge",
                "block",
                "up",
            ]),
        // View.
        Command::new("picker.files", "Go to file", VIEW).when("projectOpen"),
        /*
         * The file picker's library scope. (M16)
         *
         * # Why this is a command and not an `if (ev.altKey)` in the overlay
         *
         * Because a local key handler is unrebindable, unlisted and undiscoverable — the exact
         * shape that produces this project's most-repeated defect. Going through the registry
         * costs one entry here, one `when` flag, one default binding and one `case` in
         * `dispatch.ts`, and buys a palette row, a rebindable chord and a `check:commands` that
         * fails if any of the four is missing.
         *
         * # `projectOpen`, not `filePickerOpen`
         *
         * The two clauses do different jobs and this one is the *palette's*. `filePickerOpen`
         * gates the keyboard — see the binding in `keymap.rs` — because ⌥L must not be taken
         * from a terminal pane in every window. The palette is a different surface: the picker
         * and the palette cannot both be open, so a `when("filePickerOpen")` here would make
         * the row permanently unlistable. From the palette the command sets the scope for the
         * *next* Ctrl+P, which is a useful thing to be able to do and the reason the row exists
         * at all.
         */
        Command::new("picker.libraries", "Search External Libraries too", VIEW)
            .when("projectOpen")
            .keywords(&["dependencies", "crates", "vendor", "registry", "sources"]),
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
        // `notes` is deliberately **not** a keyword here any more: it moved to
        // `file.projectNotes`, which is the command a user typing *notes* into the palette means.
        // Leaving it on both offered two rows for one word, one of which scrolls to a drawer of
        // scratch files.
        Command::new("view.scratches", "Show Scratches", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["scratch", "buffer", "temporary", "playground"]),
        Command::new("palette.commands", "Show all commands", VIEW),
        Command::new("theme.toggle", "Toggle light/dark theme", VIEW),
        /*
         * **No `when` clause since M74**, and the removal is the fix rather than a tidy-up.
         *
         * The clause used to read `projectOpen`, because Settings opens as a *tab inside a
         * project* and with none there was nowhere to put it. There is now: a shell drawing no
         * project draws the settings screen in the frame instead (`App.tsx`'s
         * `settingsFrame`), which is the same surface reached the same way, and settings were
         * always global — the tab was a container, never a scope.
         *
         * Note what the bare clause does *not* buy, which is the point `palette.commands`
         * makes above: `OverlayHost` is mounted only with a project, so the palette cannot be
         * raised without one and these rows are in practice only ever *picked* by somebody who
         * has one. The half that now works is the bound key, which is exactly the state the
         * command is most wanted in — a window with nothing open and a preference to change.
         */
        Command::new("settings.open", "Open settings", VIEW),
        Command::new("settings.keymap", "Open keyboard shortcuts", VIEW),
        /*
         * The About box: `cide <version>` and the `claude --version` line beside it.
         *
         * `shellWindow && projectOpen`, and not the bare clause `palette.commands` carries,
         * because the clause has to name where the card can actually appear: `App.tsx` mounts
         * `OverlayHost` in the shell branch only, and only with a project. Toggled anywhere
         * else the store would hold `'about'` with nothing on screen, and `overlayOpen()`
         * would gate every `!overlayOpen` binding behind a modal nobody can see. The palette
         * cannot reach that state — it lives in the same host — but a user's `keymap.json`
         * binding can, which is who this clause is for.
         */
        Command::new("help.about", "About cide", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["version", "help", "claude"]),
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
        /*
         * The git tool window's F4. (M18)
         *
         * In `VIEW` and **not** in `GIT`, for two reasons that agree. The test below —
         * `git_commands_ask_for_a_project_and_nothing_asks_for_the_dead_repo_flag` — walks every
         * id starting `git.` and requires `projectOpen`, which this one must not carry: hiding a
         * panel that is open is meaningful whatever the project state, and a toggle that stopped
         * working when the last project closed would leave the user with a panel they cannot
         * shut. And it belongs beside `sidebar.toggle` anyway: both are "make this piece of
         * chrome go away", and a user looking for one will look where the other is.
         */
        Command::new("view.toolWindow.toggle", "Toggle the git tool window", VIEW)
            .when("shellWindow")
            .keywords(&[
                "git", "log", "bottom", "panel", "dock", "hide", "show", "collapse",
            ]),
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
        /*
         * The two M18 panels, beside the other `sidebar.*` rows rather than in the `AGENTS`
         * group with the commands that act on what they show. (M18)
         *
         * Where a *view* lives is a question about the sidebar, and a user looking for "how do I
         * open that panel" looks where `sidebar.git` and `sidebar.problems` are — the same
         * argument `view.toolWindow.toggle` above makes for sitting in `VIEW` rather than in
         * `GIT`. The group a command belongs to is the palette's heading, not a statement about
         * which subsystem implemented it.
         *
         * `shellWindow && projectOpen`, matching `sidebar.git`, `sidebar.search` and
         * `sidebar.problems` exactly: a detached-pane window has no rail to switch, and both
         * panels read something scoped to a project — the roster for one, `.cide/tasks.json` for
         * the other. Note what is *not* in either clause: whether subagents are enabled for this
         * project. See the `AGENTS` block above for why that flag must never exist, and note
         * that the Tasks panel would not want it anyway — a tracker is useful on its own, and
         * gating it would make the first thing a curious user clicks say "turn on a feature you
         * have not read about".
         */
        Command::new("sidebar.agents", "Show agents sidebar", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["subagents", "roles", "dispatch", "workers", "orchestration"]),
        Command::new("sidebar.tasks", "Show tasks sidebar", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["todo", "board", "tracker", "backlog"]),
        // M28. `projectOpen` for the reason the two above carry it: the panel is about a
        // project's files, and a detached-pane window has neither a project nor a sidebar.
        Command::new("sidebar.openspec", "Show OpenSpec sidebar", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "spec",
                "specs",
                "requirements",
                "proposal",
                "change",
                "spec-driven",
            ]),
        /*
         * Propose a change, and explore before proposing. (M28)
         *
         * Both open the composer — the same one the OpenSpec panel's two buttons open — and both
         * end in the project's pinned Claude session being typed at. They are here because that
         * is where a gesture with no home on screen goes: the panel is one click away when the
         * sidebar happens to be showing OpenSpec, and behind two otherwise, and *propose a
         * change* is the single most common thing a user of this feature does.
         *
         * `projectOpen` and not a board condition: whether a project has `openspec/` is a
         * subprocess away, `CONTEXT_FLAGS` may only name flags the webview actually supplies, and
         * a flag nobody sets is false for ever and the command silently vanishes from the palette.
         * The composer itself is what says so — it is the surface that knows the board.
         */
        Command::new("spec.propose", "OpenSpec: propose a change", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&[
                "openspec",
                "proposal",
                "change",
                "spec",
                "new change",
                "opsx",
            ]),
        Command::new("spec.explore", "OpenSpec: explore before proposing", VIEW)
            .when("shellWindow && projectOpen")
            .keywords(&["openspec", "explore", "investigate", "spec", "opsx"]),
        /*
         * The extension manager. (M22)
         *
         * `shellWindow` and **not** `projectOpen`, unlike the four sidebar rows above it. An
         * extension is global — it is installed once and wanted in every project — so the panel
         * that installs one has to be reachable from a window with nothing open, which is exactly
         * the state a user is in the first time they go looking for it.
         */
        Command::new("sidebar.extensions", "Show extensions sidebar", VIEW)
            .when("shellWindow")
            .keywords(&["marketplace", "plugins", "install", "language support"]),
        /*
         * Re-run the analysers. (M18)
         *
         * The user asked for this in as many words — *"i have not manual button to rerun it in
         * left side bar inside problems panel"* — and the Problems panel now has the button. It is
         * a command as well as a button because that is this project's rule: a gesture with no id
         * cannot be bound, cannot be found in the palette, and cannot be reached without a mouse,
         * which is the gap `sidebar.search` and `sidebar.problems` were both added to close.
         *
         * `projectOpen` and nothing narrower. A `sidebarProblems` flag would scope it to the
         * panel and would have to be *supplied* by the webview or it is false for ever — this
         * repository lost all four git commands to exactly that mistake (see the module docs on
         * `repoOpen`), and the true precondition is anyway "there is a project to analyse" rather
         * than "the panel happens to be showing".
         *
         * **No default binding.** IDEA and VS Code both ship none for their equivalents, nothing
         * here requires one, and the id is rebindable regardless — a chord spent on a control most
         * users press once a week is a chord taken from every terminal pane in every window.
         */
        // The nuclear neighbour of `problems.refresh`: stop every analyser, delete every
        // server's on-disk cache (the bundled rust-analyzer's disk index above all), start
        // them again. Distinct from a restart on purpose — a restart *restores* the disk
        // index, so when the index itself has gone wrong, restarting reproduces the problem.
        // IntelliJ users will look for this under the words "invalidate caches".
        Command::new(
            "problems.invalidateCaches",
            "Invalidate index caches and restart analysers",
            VIEW,
        )
        .when("projectOpen")
        .keywords(&[
            "invalidate",
            "caches",
            "index",
            "rebuild",
            "clear",
            "reindex",
            "corrupt",
            "lsp",
            "rust-analyzer",
        ]),
        Command::new("problems.refresh", "Re-run code analysis", VIEW)
            .when("projectOpen")
            // "rerun" and "refresh" are what a user types; "stale" is what they are looking at
            // when they go looking for it.
            .keywords(&[
                "rerun",
                "refresh",
                "rescan",
                "diagnostics",
                "problems",
                "lsp",
                "stale",
                "analyse",
            ]),
        /*
         * Code folding. (M19)
         *
         * # Why `VIEW` and not a new `Code` group
         *
         * IDEA files these under *Code ▸ Folding* and a `CODE` constant would read better in the
         * palette. It loses on what a group *is* here: `groups_are_listed_once_each_in_table_order`
         * pins the whole list, so a tenth group is a deliberate decision about the shape of the
         * palette, and folding does not earn one on its own — collapsing a block changes what is
         * on screen and changes nothing about the file, which is the definition `VIEW` already
         * holds for the sidebar toggles and the theme.
         *
         * # Why seven and not three
         *
         * Because the three-command version is the one that does not work. Collapse-all leaves a
         * file where every expand needs a second expand for the level below it, and IDEA's answer
         * — the recursive pair — is what makes that navigable. They are cheap: all seven are one
         * `case` each over the same `FoldActions` bundle, and a command nobody binds costs a row
         * in Settings ▸ Keymap.
         *
         * All seven are `editorFocused`, and the *handlers* re-check it: a clause here filters
         * the palette and never gates the keyboard (see this module's header), so the arm in
         * `ui/src/keys/dispatch.ts` asks `focusedFolds()` itself and reports a refusal when the
         * answer is null.
         */
        Command::new("editor.fold", "Collapse", VIEW)
            .when("editorFocused")
            .keywords(&["fold", "collapse", "hide", "block"]),
        Command::new("editor.unfold", "Expand", VIEW)
            .when("editorFocused")
            .keywords(&["unfold", "expand", "open", "block"]),
        Command::new("editor.toggleFold", "Toggle fold", VIEW)
            .when("editorFocused")
            .keywords(&["fold", "collapse", "expand", "toggle"]),
        Command::new("editor.foldAll", "Collapse all", VIEW)
            .when("editorFocused")
            .keywords(&["fold", "collapse", "everything", "file"]),
        Command::new("editor.unfoldAll", "Expand all", VIEW)
            .when("editorFocused")
            .keywords(&["unfold", "expand", "everything", "file"]),
        Command::new("editor.foldRecursively", "Collapse recursively", VIEW)
            .when("editorFocused")
            .keywords(&["fold", "collapse", "nested", "children"]),
        Command::new("editor.unfoldRecursively", "Expand recursively", VIEW)
            .when("editorFocused")
            .keywords(&["unfold", "expand", "nested", "children"]),
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
        // that is where the M12 block was inserted, not because anything sorted it. "Agents"
        // follows "Claude" for the opposite reason: it was put there on purpose, next to the
        // group it deliberately is not part of, because a user who has just read "Fork session
        // into new pane" is one row away from the subagents that are the other way to get work
        // done in this app.
        assert_eq!(
            groups(),
            vec![
                "Window", "Project", "Claude", "Agents", "Terminal", "File", "Git", "Docker",
                "Navigate", "View",
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
    fn claude_commands_require_a_claude_pane_except_the_ones_that_do_not_act_on_a_session() {
        for command in registry().iter().filter(|c| c.id.starts_with("claude.")) {
            let expected = match command.id.as_str() {
                // "Mention the file I am looking at", which means an editor has focus and so
                // a Claude pane does not. Requiring `claudePaneFocused` made it a row that
                // could only ever be offered in the one state where it has no file to
                // mention.
                "claude.mention.file" => "editorFocused && claudeTarget",
                // Each creates the session it splits to, so it needs a pane to split and not
                // a conversation to act on — the same clause `terminal.splitBelow` carries.
                "claude.split.newSession" | "claude.split.right" | "claude.addRow" => "paneFocused",
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
            &ids(&found)[..6],
            &[
                "pane.split.right",
                "pane.split.down",
                "claude.split.newSession",
                "claude.split.right",
                "terminal.splitBelow",
                "terminal.splitRight",
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
        // And on its own, through the keyword tier — `create` is a declared keyword of
        // `git.branch.new` and a bare subsequence of a couple of unrelated titles, so the
        // keyword tier is what puts it first.
        //
        // The M18 tag command was very nearly `git.tag.create`, and it would have taken this
        // position: an id *containing* the word scores on [`TIER_ID`], a tier above keywords, so
        // *Tag commit…* would have led and *New branch…* would have followed. Nobody typing
        // `create` into this palette means "tag". The id is `git.tag.new` instead — which also
        // matches `git.branch.new`, so the two commands that make a new ref are named alike.
        // Ids are API, so this was worth getting right before it shipped rather than after.
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
    fn a_title_prefix_outranks_a_word_later_in_a_title() {
        // Named for what it measures. It was `..._outranks_a_title_substring` and never touched
        // the substring tier: `window` matches `Detach pane into window` at a *word* boundary,
        // which is the tier above. The substring tier is gone now — see the note beside
        // `TIER_ID` — and this comparison is unchanged by that, which is the point of saying so.
        let found = ids(&search("window"));
        let stacked = found.iter().position(|id| *id == "window.mode.stacked");
        let detach = found.iter().position(|id| *id == "pane.detachToWindow");
        assert!(
            stacked < detach,
            "`Window mode: stacked` should beat `Detach pane into window`: {found:?}"
        );
    }

    #[test]
    fn a_word_inside_another_word_does_not_outrank_a_declared_keyword() {
        // The regression M48 paid for, pinned from both ends.
        //
        // `Compose Recreate` contains `create` inside a word; `New branch…` declares it as a
        // keyword. The title tiers used to include a substring rung above keywords, so adding
        // the Docker group silently moved `New branch…` down — a palette answer that got worse
        // because an unrelated feature landed.
        let found = ids(&search("create"));
        assert_eq!(found[0], "git.branch.new", "{found:?}");

        // And the other end: the Compose row is still *offered*, one tier lower, through the
        // subsequence rung. Removing the tier must not remove the command from the list.
        assert!(found.contains(&"docker.compose.recreate"), "{found:?}");

        // The whole-word claim is untouched — `recreate` still finds it first.
        assert_eq!(ids(&search("recreate"))[0], "docker.compose.recreate");
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
