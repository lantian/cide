//! A language, as data. (M22)
//!
//! Everything `ui/src/editor/` needs in order to colour, fold and name a file, in a shape that
//! can be written in a JSON manifest by somebody who has never seen this repository.
//!
//! # Why this exists at all
//!
//! Until M22 a language was five edits: a member of a closed `LanguageId` union, a row in
//! `REGISTRY`, a row in `BY_EXTENSION`, an entry in a `Record<LanguageId, FoldSpec>` that the
//! type checker made exhaustive, and a `ui/src/editor/languages/<id>.ts` module. Four of those
//! five are pure data and the fifth nearly is — `ui/src/editor/streamGrammar.ts`'s `GrammarSpec`
//! is a record of keyword tables and delimiter characters with exactly one function-valued field.
//! So the closed union was never buying type safety over anything that mattered; it was buying
//! it over a table that could just as well have been a list.
//!
//! This module is that list's element type. The **builtin** languages and an **extension's**
//! languages are the same shape and go through the same registry, on the rule
//! `cide-core::commands` already follows for commands and keys: two tables would let a language
//! be highlightable but unfoldable, or foldable under a name the status bar does not use, and
//! both would drift in silence.
//!
//! # The one thing an extension cannot have
//!
//! [`GrammarSpecDto`] mirrors `GrammarSpec` field for field except `hook` — the escape hatch a
//! builtin uses for the genuinely irregular parts of a language, which is a *function*. Rust's
//! lifetimes, Go's rune literals and Markdown's headings all live there, and no JSON can express
//! them.
//!
//! In its place an extension gets [`GrammarRule`]: a regex, a tag name, and whether it is tried
//! only at the head of a line. That is deliberately less than a hook and deliberately enough for
//! the shape of hook that turns up over and over — *"this pattern, in this position, is that
//! colour"*. YAML's whole hook is three of them, and translating it faithfully is the check that
//! the rule language is sufficient rather than merely plausible.
//!
//! Builtins keep their hooks. Forcing `rust.ts` to describe lifetimes as a regex list would be a
//! real regression bought for symmetry, and symmetry is not the goal — one registry is.

//! # Why nothing here uses `deny_unknown_fields`
//!
//! Every other inbound type in this crate does, on the rule the crate header states: a
//! frontend/backend drift should fail loudly rather than silently drop a field. These types are
//! not inbound from the frontend — they are read out of a **third-party manifest**, and there the
//! right answer is the opposite one. A manifest written for a newer cide should still install what
//! this one understands, so an unknown key is a *warning naming the key*, produced by
//! `cide_ext::manifest`'s own pre-pass over the raw `Value` where a line number is still available.
//! `deny_unknown_fields` here would turn every such manifest into a hard refusal with serde's
//! wording instead of cide's.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where in a line a [`GrammarRule`] is allowed to match.
///
/// Not a refinement: it is what keeps a rule from being quadratic. `data.ts`'s YAML key pattern
/// scans to the end of the line before its lookahead can fail, so running it once per token makes
/// a 200,000-character line 200,000 scans of 200,000 characters — measured at 4.8 s against 20 ms
/// for the same line tried at the line head only. CodeMirror parses under a time budget, so that
/// never froze a window; it just meant the colour never arrived and a core burned until the
/// buffer was closed. A manifest author will not know that, which is exactly why the choice is a
/// field with two values rather than a comment asking them to be careful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RuleAt {
    /// Only when nothing but whitespace precedes this token on its line.
    ///
    /// `atLineStart` in `streamGrammar.ts`, not `stream.sol()`: the generic path eats leading
    /// whitespace before a hook runs, so by the time an indented `key:` is seen the position is
    /// past column 0 and `sol()` is already false.
    LineStart,
    /// Anywhere in the line. Keep the pattern anchored and cheap.
    Anywhere,
}

/// One *"this pattern is that colour"* rule, compiled into a grammar hook by the frontend.
///
/// # The pattern
///
/// A JavaScript regular expression **source** — no delimiters, no flags. The frontend anchors it
/// with `^` if the author did not, because `StringStream.match` matches at the stream position
/// and an unanchored pattern would silently scan forward and colour the wrong run.
///
/// It **must consume at least one character when it matches**. `StreamLanguage` throws
/// *"Stream parser failed to advance stream"* after ten no-op calls, which does not mis-colour a
/// buffer, it takes the buffer down. A zero-width match is refused at load with the manifest's
/// path and the rule's index, rather than at the tenth token of the first file somebody opens.
///
/// # The tag
///
/// A CodeMirror stream-parser token name — `propertyName`, `meta`, `labelName`,
/// `variableName.function`. Validated in the frontend rather than here, against
/// `editor/highlight.ts`'s `tagsFor`, which is the same function the buffer and the minimap
/// resolve names through. A list of legal names copied into this crate would be a second
/// authority that could disagree with the first, and the disagreement would show as a token that
/// parses, matches, and paints in no colour at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrammarRule {
    /// A JavaScript regex source. See the type's own note — it must consume a character.
    pub pattern: String,
    /// The stream-parser token name this run is coloured as.
    pub tag: String,
    /// Where the rule may match. Defaults to [`RuleAt::Anywhere`].
    #[serde(default = "anywhere")]
    pub at: RuleAt,
}

fn anywhere() -> RuleAt {
    RuleAt::Anywhere
}

/// The tokenizer, as data. Mirrors `ui/src/editor/streamGrammar.ts`'s `GrammarSpec`.
///
/// Field for field, minus `hook` and plus [`GrammarRule`]s — see the module header. The mirroring
/// is load-bearing in one direction that is easy to miss: six of these fields are *also* mirrored
/// by [`FoldSpecDto`], and `ui/scripts/check-editor.mjs` asserts the two agree. A grammar that
/// thought `#` opened a comment in Rust would treat every `#[derive(…)]` as a dead line, and
/// nothing on screen would say so.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GrammarSpecDto {
    /// What the status bar readout calls it: `SQL · UTF-8 · LF · …`.
    pub name: String,
    pub keywords: Vec<String>,
    /// Whether `SELECT`, `select` and `Select` are all the same word. SQL wants this; Go must not
    /// have it, because `String` and `string` are two different things.
    pub case_insensitive_keywords: bool,
    pub types: Vec<String>,
    pub atoms: Vec<String>,
    pub builtins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub block_comment: Option<(String, String)>,
    pub nested_comments: bool,
    /// Characters that open a string. `None` means the engine's default of `"` and `'`; `Some("")`
    /// means none at all, which is what Markdown wants so an apostrophe in *don't* opens nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quotes: Option<String>,
    /// Whether a backslash escapes the next character inside a string. `None` is the engine's
    /// default of `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub escapes: Option<bool>,
    pub triple_quotes: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identifier_extra: Option<String>,
    /// Whether an initial capital reads as a type. Wrong for shell and YAML.
    pub capitalised_is_type: bool,
    /// Whether `name(` reads as a call. Wrong for shell, where `(` opens a subshell.
    pub call_syntax: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub control_operators: Option<String>,
    /// Tried in order, before everything else, once per token. See [`GrammarRule`].
    pub rules: Vec<GrammarRule>,
}

/// What the fold scanner needs. Mirrors `ui/src/editor/foldRanges.ts`'s `FoldSpec`.
///
/// A second record rather than a view over [`GrammarSpecDto`], for the reason `languages.ts`
/// already gives: a grammar arrives through a dynamic `import()`, and fold restore has to run
/// inside the editor's **mount dispatch** — the remembered scroll position is a line number, and
/// applying it against unfolded heights and folding a tick later lands the reader somewhere they
/// did not leave. So this is available synchronously and the grammar is not.
///
/// For an extension that means the whole resolved language set has to be present at first paint,
/// which is why it rides [`crate::Bootstrap`] rather than being fetched after the window opens.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct FoldSpecDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub block_comment: Option<(String, String)>,
    /// `Option` and not `bool`, unlike every other flag here, because this field **mirrors** the
    /// grammar's and `ui/scripts/check-editor.mjs` asserts the two are equal. Go and SQL write
    /// `nestedComments: false` in their grammars on purpose — C-style comments that do not nest —
    /// and JSON writes nothing at all; collapsing those two states here would make the mirror
    /// inexact for one of them whichever way the collapse went. The same is true of
    /// [`Self::triple_quotes`] and [`Self::escapes`].
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nested_comments: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quotes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub escapes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub triple_quotes: Option<bool>,
    /// The opening characters that make a block. `None` is the scanner's default of `{[(`;
    /// `Some("")` disables bracket folding.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub brackets: Option<String>,
    /// Quote characters whose strings may cross a line break. The default is **none**, which is
    /// the opposite of what the grammar does and is the safe direction: a wrongly-coloured tail is
    /// cosmetic, but a `{` wrongly inside a string silently deletes folding for the rest of the
    /// buffer.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub multiline_quotes: Option<String>,
    /// Quote characters the grammar opens in its *hook* rather than through `quotes`. Builtins
    /// only — an extension has no hook, so a quote it wants the scanner to know about belongs in
    /// `quotes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub extra_quotes: Option<String>,
    /// Rust's `'a`. Builtin-only, like `raw_strings`: both name a construct the fold scanner
    /// hard-codes, and neither is expressible as a manifest field for a language it has never
    /// heard of.
    pub lifetimes: bool,
    /// Rust's `r#"…"#`.
    pub raw_strings: bool,
    /// Fold by indentation as well: Python's suites, YAML's mappings.
    pub indent_blocks: bool,
    /// Fold `#` headings to the next heading of the same or higher level.
    pub heading_folds: bool,
    /// Fold fenced blocks, and do not read headings inside one.
    pub fenced_blocks: bool,
    /// Honour `region` / `endregion` markers. Needs `line_comment`.
    pub regions: bool,
}

/// A row the *New scratch file…* picker offers for this language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScratchTypeDto {
    /// The row's label, and what the status bar will say once the file is open.
    pub label: String,
    /// The extension, without the dot. Must be one of this language's own `extensions`, or the
    /// picker would offer a type that opens as plain text — `check-editor.mjs` makes that a build
    /// failure for the builtins and the frontend refuses it for an extension.
    pub ext: String,
}

/// One extension-to-language mapping, with the label that mapping is shown under.
///
/// The label override is why this is a record and not a bare string: a `.tsx` file reads `TSX` in
/// the tab badge even though it loads the TypeScript table, and the two disagreeing would look
/// like a bug rather than a shorthand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileExtension {
    /// Lowercase, without the dot.
    pub ext: String,
    /// What the status bar calls a file with this extension. Defaults to the language's `label`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}

/// A whole language: what names it, what colours it, and how it folds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanguageDef {
    /// A registry identifier, not a user-visible name — `sql`, `yaml`, `clike`.
    ///
    /// Also the `languageId` a language server's documents carry, which is why it is spelled the
    /// way LSP spells it rather than however the author would like it capitalised.
    pub id: String,
    /// What the status bar readout calls it.
    pub label: String,
    /// The file extensions this language claims, lowercase and without the dot.
    #[serde(default)]
    pub extensions: Vec<FileExtension>,
    /// Whole file names with no useful extension — `dockerfile`, `.bashrc`. Lowercased on lookup.
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Markdown fence words that mean this language, spelled as *extensions* — `golang` → `go` —
    /// so they route through the same lookup a file path does.
    #[serde(default)]
    pub fence_aliases: Vec<String>,
    pub grammar: GrammarSpecDto,
    #[serde(default)]
    pub fold: FoldSpecDto,
    /// Rows for the *New scratch file…* picker, in the order it should list them.
    ///
    /// A list and not one row, because a language can be two things a person asks for: `.ts` and
    /// `.js` load the same table, and so do `.c` and `.cpp`. Ordinary extensions want one.
    #[serde(default)]
    pub scratch: Vec<ScratchTypeDto>,
}

/// A language server cide can drive.
///
/// Until M22 this was `cide_lsp::Server`, a two-member enum with six `match` arms and — the field
/// that turns out to matter most — **no arguments at all**: `server.rs` built `Command::new(bin)`
/// and spawned it. `rust-analyzer` and `gopls` both speak LSP on stdio with no flags, so nothing
/// noticed. `yaml-language-server` needs `--stdio` and says so in its own README.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanguageServerDef {
    /// The binary name, looked up on the child's `PATH`. Never an absolute path from a manifest:
    /// a marketplace that could name `/usr/bin/…` could name anything.
    pub binary: String,
    /// Arguments, in order. `["--stdio"]` for the node-based servers.
    #[serde(default)]
    pub args: Vec<String>,
    /// The [`LanguageDef::id`]s whose files this server owns.
    pub language_ids: Vec<String>,
    /// A file whose presence means a directory is a project worth analysing.
    ///
    /// Empty means *any root* — which is right for a server that analyses single files (`sqls`
    /// has no manifest to look for) and wrong for anything that resolves a dependency graph.
    #[serde(default)]
    pub project_markers: Vec<String>,
    /// What the Problems panel calls a root this server claimed: `Cargo`, `Go module`.
    pub project_kind: String,
    /// How to install it, in the words the user would type.
    pub install_hint: String,
    /// Whether the server declares `workspace/didChangeWatchedFiles` and should be sent them.
    ///
    /// `gopls` has no watcher of its own and needs them; `rust-analyzer` has one that sees more
    /// than cide's gitignore-filtered index and is made worse by being told twice.
    #[serde(default)]
    pub declares_watched_files: bool,
    /// Directories to add to the child's `PATH` when the binary is not otherwise findable —
    /// `~/.cargo/bin`, `~/go/bin`, `~/.local/share/npm/bin`. `~` is expanded; nothing else is.
    #[serde(default)]
    pub extra_path_hints: Vec<String>,
}

// ==========================================================================================
// The builtin languages.
// ==========================================================================================

/// Every language compiled into cide.
///
/// # Why this is in Rust when the tokenizers are in TypeScript
///
/// Because *routing* and *tokenizing* are two different facts, and only one of them can be
/// written down twice without anybody noticing.
///
/// A builtin's tokenizer contains a **function** — Rust's lifetimes, Go's rune literals, Markdown's
/// headings — so it cannot be data and stays where it is, one module per language under
/// `ui/src/editor/languages/`. Everything else about a language is a table: which extensions it
/// claims, what the status bar calls it, how it folds, whether the scratch picker offers it. Those
/// tables have to agree with the ones an extension contributes, because they are merged into one
/// lookup, and the only way to be sure they agree is for there to be one of them.
///
/// So this is the table, and `ui/src/editor/builtinLanguages.ts` is generated from it by
/// `cargo xtask codegen` — which means `codegen --check` fails the build if the two drift, in
/// exactly the way it already does for `ui/src/ipc/generated.ts`.
///
/// It also means the Rust side can answer *"which language is this path"* without asking the
/// webview, which is what M22 needed in the first place: `cide-app`'s `server_for` used to derive
/// a language server from `cide_lang::Lang` — a closed enum backed by statically linked C parser
/// tables — so a language cide could run a server for and a language cide could outline were
/// forced to be the same set. A YAML server needed that coupling broken, and this is what breaks
/// it.
///
/// # The order is the scratch picker's order
///
/// `filterScratchTypes` lists rows in registry order, so this list is ordered by *what this
/// application is for* rather than alphabetically, and the eleven entries reproduce the fourteen
/// rows `SCRATCH_TYPES` has offered since M15 exactly. `Plain Text` is not here and must not be:
/// it is the picker's deliberate exception, a row whose extension resolves to no language at all,
/// and giving it one would be giving it a grammar it does not have.
#[must_use]
pub fn builtins() -> Vec<LanguageDef> {
    vec![
        LanguageDef {
            id: "rust".into(),
            label: "Rust".into(),
            extensions: exts(&[("rs", None)]),
            filenames: vec![],
            fence_aliases: words(&["rust"]),
            grammar: named("Rust"),
            fold: FoldSpecDto {
                line_comment: Some("//".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                nested_comments: Some(true),
                // A `"` genuinely spans lines in Rust, and `raw_strings` is here rather than
                // anywhere else because `r#"…"#` is where an unbalanced brace most often lives —
                // a `format!` template, an embedded shell script, a regex.
                multiline_quotes: Some("\"".into()),
                raw_strings: true,
                // `fn f<'a>(x: &'a str) {` — without this the `'` opens a string, the trailing
                // `{` is inside it, and the function is not foldable. In a codebase with
                // lifetimes that is most of it.
                lifetimes: true,
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("Rust", "rs")]),
        },
        LanguageDef {
            id: "go".into(),
            label: "Go".into(),
            extensions: exts(&[("go", None)]),
            filenames: vec![],
            fence_aliases: words(&["golang"]),
            grammar: named("Go"),
            fold: FoldSpecDto {
                line_comment: Some("//".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                nested_comments: Some(false),
                quotes: Some("\"".into()),
                extra_quotes: Some("`".into()),
                multiline_quotes: Some("`".into()),
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("Go", "go")]),
        },
        LanguageDef {
            id: "typescript".into(),
            label: "TypeScript".into(),
            extensions: exts(&[
                ("ts", None),
                ("mts", None),
                ("cts", None),
                ("tsx", Some("TSX")),
                ("js", Some("JavaScript")),
                ("mjs", Some("JavaScript")),
                ("cjs", Some("JavaScript")),
                ("jsx", Some("JSX")),
            ]),
            filenames: vec![],
            fence_aliases: words(&["typescript", "javascript"]),
            grammar: named("TypeScript"),
            fold: FoldSpecDto {
                line_comment: Some("//".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                extra_quotes: Some("`".into()),
                multiline_quotes: Some("`".into()),
                regions: true,
                ..FoldSpecDto::default()
            },
            // Two rows for one language, which is why `scratch` is a list: a `.ts` and a `.js`
            // scratch load the same table and are two different things to ask for.
            scratch: scratch(&[("TypeScript", "ts"), ("JavaScript", "js")]),
        },
        LanguageDef {
            id: "python".into(),
            label: "Python".into(),
            extensions: exts(&[("py", None), ("pyi", None)]),
            filenames: vec![],
            fence_aliases: words(&["python"]),
            grammar: named("Python"),
            fold: FoldSpecDto {
                line_comment: Some("#".into()),
                triple_quotes: Some(true),
                indent_blocks: true,
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("Python", "py")]),
        },
        LanguageDef {
            id: "json".into(),
            label: "JSON".into(),
            extensions: exts(&[("json", None), ("jsonc", None)]),
            filenames: vec![],
            fence_aliases: words(&["jsonc"]),
            grammar: named("JSON"),
            fold: FoldSpecDto {
                line_comment: Some("//".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                quotes: Some("\"".into()),
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("JSON", "json")]),
        },
        LanguageDef {
            id: "yaml".into(),
            label: "YAML".into(),
            extensions: exts(&[("yaml", None), ("yml", None)]),
            filenames: vec![],
            fence_aliases: words(&["yml"]),
            grammar: named("YAML"),
            fold: FoldSpecDto {
                line_comment: Some("#".into()),
                quotes: Some("\"'".into()),
                indent_blocks: true,
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("YAML", "yaml")]),
        },
        LanguageDef {
            id: "toml".into(),
            label: "TOML".into(),
            extensions: exts(&[("toml", None), ("lock", Some("TOML"))]),
            filenames: words(&[".gitconfig"]),
            fence_aliases: vec![],
            grammar: named("TOML"),
            fold: FoldSpecDto {
                line_comment: Some("#".into()),
                quotes: Some("\"'".into()),
                triple_quotes: Some(true),
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("TOML", "toml")]),
        },
        LanguageDef {
            id: "markdown".into(),
            label: "Markdown".into(),
            extensions: exts(&[("md", None), ("markdown", None)]),
            filenames: vec![],
            fence_aliases: vec![],
            grammar: named("Markdown"),
            fold: FoldSpecDto {
                // No strings and no brackets: a `(` in prose is not a block, and `quotes: ""` is
                // what the grammar says too — an apostrophe in *don't* would otherwise open a
                // string.
                quotes: Some(String::new()),
                brackets: Some(String::new()),
                heading_folds: true,
                fenced_blocks: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("Markdown", "md")]),
        },
        LanguageDef {
            id: "shell".into(),
            label: "Shell".into(),
            extensions: exts(&[("sh", None), ("bash", None), ("zsh", None), ("fish", None)]),
            filenames: words(&[".bashrc", ".zshrc", ".profile", "dockerfile", "makefile"]),
            fence_aliases: words(&["shell", "console", "shell-session", "sh-session", "zsh"]),
            grammar: named("Shell"),
            fold: FoldSpecDto {
                line_comment: Some("#".into()),
                // Both quotes span lines in every shell, which is not true of most languages and
                // is why the scanner's default is the other way round.
                multiline_quotes: Some("\"'".into()),
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("Shell", "sh")]),
        },
        LanguageDef {
            id: "sql".into(),
            label: "SQL".into(),
            extensions: exts(&[("sql", None)]),
            filenames: vec![],
            fence_aliases: vec![],
            grammar: named("SQL"),
            fold: FoldSpecDto {
                line_comment: Some("--".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                nested_comments: Some(false),
                quotes: Some("'\"`".into()),
                escapes: Some(false),
                multiline_quotes: Some("'".into()),
                regions: true,
                ..FoldSpecDto::default()
            },
            scratch: scratch(&[("SQL", "sql")]),
        },
        LanguageDef {
            id: "clike".into(),
            label: "C-like".into(),
            extensions: exts(&[
                ("c", Some("C")),
                ("h", Some("C")),
                ("cc", Some("C++")),
                ("cpp", Some("C++")),
                ("cxx", Some("C++")),
                ("hpp", Some("C++")),
                ("java", Some("Java")),
            ]),
            filenames: vec![],
            fence_aliases: words(&["c++", "objc"]),
            grammar: named("C-like"),
            fold: FoldSpecDto {
                line_comment: Some("//".into()),
                block_comment: Some(("/*".into(), "*/".into())),
                regions: true,
                ..FoldSpecDto::default()
            },
            // `C-like` is never offered under that name: the module is reached under six labels
            // and "C-like" is an implementation detail, not something a person picks.
            scratch: scratch(&[("C", "c"), ("C++", "cpp")]),
        },
    ]
}

/// A grammar carrying only its name.
///
/// A builtin's tokenizer is compiled into the frontend, so what belongs here is the one field the
/// status bar reads before the chunk has resolved — and nothing else. An empty `rules` list is
/// what tells the frontend registry *"the grammar for this id is a module, go and import it"*,
/// where a contributed language's non-empty one says *"build it from this"*.
fn named(name: &str) -> GrammarSpecDto {
    GrammarSpecDto {
        name: name.to_string(),
        ..GrammarSpecDto::default()
    }
}

fn exts(rows: &[(&str, Option<&str>)]) -> Vec<FileExtension> {
    rows.iter()
        .map(|(ext, label)| FileExtension {
            ext: (*ext).to_string(),
            label: label.map(ToString::to_string),
        })
        .collect()
}

fn scratch(rows: &[(&str, &str)]) -> Vec<ScratchTypeDto> {
    rows.iter()
        .map(|(label, ext)| ScratchTypeDto {
            label: (*label).to_string(),
            ext: (*ext).to_string(),
        })
        .collect()
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// The language servers compiled into cide.
///
/// Until M22 this was `cide_lsp::Server`, a closed two-member enum with six `match` arms spread
/// over three files. Nothing about rust-analyzer or gopls has changed; what has changed is that
/// the arms are now rows, so a third one can arrive from a manifest instead of from a rebuild.
#[must_use]
pub fn builtin_servers() -> Vec<LanguageServerDef> {
    vec![
        LanguageServerDef {
            binary: "rust-analyzer".into(),
            args: vec![],
            language_ids: words(&["rust"]),
            project_markers: words(&["Cargo.toml"]),
            project_kind: "Cargo".into(),
            install_hint: "rustup component add rust-analyzer".into(),
            // rust-analyzer has a watcher of its own that sees more than cide's
            // gitignore-filtered index, and telling it twice makes it worse rather than better.
            declares_watched_files: false,
            extra_path_hints: words(&["~/.cargo/bin"]),
        },
        LanguageServerDef {
            binary: "gopls".into(),
            args: vec![],
            language_ids: words(&["go"]),
            project_markers: words(&["go.mod", "go.work"]),
            project_kind: "Go module".into(),
            install_hint: "go install golang.org/x/tools/gopls@latest".into(),
            // gopls has no watcher and needs to be told.
            declares_watched_files: true,
            extra_path_hints: words(&["~/go/bin"]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emit the builtin table for `cargo xtask codegen`.
    ///
    /// Beside the `ts-rs` exporters and run by the same `cargo test -p cide-ipc`, because xtask
    /// already shells out to that and reads what it left behind. A `.json` rather than a `.ts`
    /// so `refresh_bindings` — which clears `*.ts` before every run — leaves it alone.
    #[test]
    fn export_builtin_languages() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
        std::fs::create_dir_all(&dir).expect("bindings dir");
        let payload = serde_json::json!({
            "languages": builtins(),
            "servers": builtin_servers(),
        });
        std::fs::write(
            dir.join("builtins.json"),
            serde_json::to_vec_pretty(&payload).expect("serialise"),
        )
        .expect("write builtins.json");
    }

    /// Two languages claiming one extension would make the merged lookup order-dependent, and
    /// the order of a `vec!` literal is not a thing anybody reviews.
    #[test]
    fn no_two_builtins_claim_the_same_extension() {
        let mut seen: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for lang in builtins() {
            for ext in &lang.extensions {
                if let Some(other) = seen.insert(ext.ext.clone(), lang.id.clone()) {
                    panic!("`{}` is claimed by both {other} and {}", ext.ext, lang.id);
                }
            }
            for name in &lang.filenames {
                if let Some(other) = seen.insert(format!("name:{name}"), lang.id.clone()) {
                    panic!("`{name}` is claimed by both {other} and {}", lang.id);
                }
            }
        }
    }

    /// A scratch row whose extension the language does not claim would be offered as `YAML` and
    /// open as plain text — the failure `SCRATCH_TYPES`' header calls silent, specific and
    /// invisible to `tsc`. `check-editor.mjs` asserts it end to end; this catches it here, where
    /// the table is written.
    #[test]
    fn every_scratch_row_resolves_to_its_own_language() {
        for lang in builtins() {
            for row in &lang.scratch {
                assert!(
                    lang.extensions.iter().any(|e| e.ext == row.ext),
                    "{} offers .{} and does not claim it",
                    lang.id,
                    row.ext
                );
            }
        }
    }

    /// A server whose language nothing contributes would never run. Builtins may not have that
    /// excuse; an extension may, and warns instead — see `cide_ext::manifest`.
    #[test]
    fn every_builtin_server_names_a_builtin_language() {
        let ids: Vec<String> = builtins().into_iter().map(|l| l.id).collect();
        for server in builtin_servers() {
            for id in &server.language_ids {
                assert!(ids.contains(id), "{} claims unknown `{id}`", server.binary);
            }
        }
    }
}
