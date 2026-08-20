//! What roles a project has: `.cide/agents/<name>.md`, read, merged, validated. (M18)
//!
//! # Why markdown with front matter, when every other cide disk format is JSON
//!
//! Because the body *is* a system prompt: a multi-paragraph document, written and re-written by
//! a human, reviewed by their team in a pull request. A system prompt inside a JSON string is
//! one line with `\n` between every sentence — unreadable in an editor and, decisively,
//! unreviewable in a diff, where a two-word change to paragraph four shows up as the whole
//! prompt replaced. `workspace.json` is machine-written state that no one reads by hand and that
//! argument does not apply to it; this file is the opposite of that, and the difference outranks
//! being consistent with the rest of cide's disk formats.
//!
//! The switches around the prompt (`harness`, `tools`, `permission-mode`) are flat scalars, so
//! they fit in front matter without wanting any of YAML's structure — which is the observation
//! the parser below is built on.
//!
//! # Why the front-matter parser is written by hand
//!
//! Three reasons, in the order they decided it.
//!
//! * **`serde_yaml` is deprecated and unmaintained.** Taking a dependency whose author has said
//!   it is finished, to read eight keys, is a maintenance bill with no upside.
//! * **A full YAML parser is an enormous surface for eight flat keys.** Anchors, aliases, tags,
//!   block scalars, flow collections, merge keys, implicit typing (`no` is `false`, `1.0` is a
//!   float, `NO` is a country) — none of which any definition file will ever use, all of which
//!   would then be part of what cide promises to read the same way next year.
//! * **A restricted grammar can name the file *and the line*.** That is the whole point. A YAML
//!   error generally cannot: it reports a position inside a document it has already partly
//!   restructured, and the message is about the parser's state machine rather than about what
//!   the author typed. Every refusal below carries `path` and `line` so the panel can say
//!   *`.cide/agents/qa.md:4: nested values are not supported`* and the user is one click from
//!   the fix.
//!
//! This follows `cide_core::claude_cli`, which hand-rolls its own verdict tables over `claude`'s
//! argv for the same class of reason: a small, closed, well-understood grammar is cheaper to own
//! than to depend on, and it can produce a better error than a general parser.
//!
//! **What is not understood is refused, never ignored.** Indentation, block sequences, anchors,
//! aliases, tags, block scalars and flow mappings each get their own sentence. Silently
//! accepting a line this parser did not actually read is the worst available outcome: the file
//! would say one thing, the running agent would do another, and nothing anywhere would connect
//! the two.
//!
//! # A broken file never stops the others loading
//!
//! Every finding is an [`AgentProblem`] and loading continues. This is the discipline
//! `cide_fs::filter::Filter::build` uses for an unreadable `.gitignore` and
//! `cide_core::persist::load` for a broken workspace, and it matters more here than in either:
//! these files are *edited by hand and by models*, so one of them being mid-edit is the normal
//! state of the directory, and a loader that answered "no roles" to a stray tab character would
//! empty the panel every few minutes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cide_ipc::agents::{AgentDraft, AgentDraftProblem, AgentField, AgentLocation, AgentScope};
use cide_ipc::{AgentDef, AgentId, Harness};

// ==========================================================================================
// Findings.
// ==========================================================================================

/// Whether a finding stopped something.
///
/// Not in the plan's original `{ path, line, message }` sketch, and added because two of the
/// required behaviours cannot be expressed without it: an unknown tool name **warns** while an
/// unknown permission mode **greys the role**. Drawn identically they would be indistinguishable,
/// and a user would spend an afternoon chasing a yellow line that was never going to be the
/// reason their agent would not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something was refused: a file did not load, or a role loaded greyed.
    Error,
    /// Something was accepted with a note. The role still dispatches.
    Warning,
}

/// One thing wrong with one definition file.
///
/// `line` is `Option` because not every finding has one: "these two files declare the same
/// name" is a fact about a *pair* of files and belongs to neither's line 4. Inventing a line
/// number for it would send the user to an innocent line, which is worse than sending them to
/// the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProblem {
    /// The file the finding is about. Absolute, as read.
    pub path: PathBuf,
    /// 1-based line within that file, when the finding has one.
    pub line: Option<u32>,
    pub severity: Severity,
    /// One sentence, written for the person who has the file open.
    pub message: String,
}

impl AgentProblem {
    fn error(path: &Path, line: Option<u32>, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            severity: Severity::Error,
            message: message.into(),
        }
    }

    fn warning(path: &Path, line: Option<u32>, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

// ==========================================================================================
// Somebody else's vocabularies.
// ==========================================================================================

/// The values `--permission-mode` accepts, read out of `claude --help` rather than remembered.
///
/// **This is somebody else's vocabulary and it will go stale.** The Claude CLI updates itself
/// underneath a running cide; a mode added next month is a mode this list does not have. That is
/// accepted knowingly, because the alternative — accepting whatever string is in the file and
/// handing it to the child — turns a typo in the one switch that decides whether an unattended
/// process may edit files without asking into a silent behaviour change. See
/// [`Parsed::permission_mode`] for what an unrecognised value actually costs: the role loads and
/// is greyed with the list in the sentence, so the fix is ten seconds and nothing vanishes.
pub const PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "manual",
    "dontAsk",
    "plan",
];

/// The permission mode that lets a child skip every prompt.
///
/// Named as a constant because two places compare against it — the parser, which warns, and
/// `crate::dispatch_refusal`, which refuses — and a string literal in both is how the two come
/// to disagree.
pub const BYPASS_PERMISSIONS: &str = "bypassPermissions";

/// Tool names the Claude CLI ships with, for the purpose of catching a typo.
///
/// **Advisory only, and an unknown name is a warning, never a refusal.** This list is the CLI's
/// and it moves between releases: tools are added, renamed and split, and cide has no way to
/// enumerate the set a given `claude` build actually has. A role naming `Notebook` when the
/// release calls it `NotebookEdit` should say so on the row and still run — refusing it would
/// mean a CLI update could empty a user's roster.
///
/// MCP tools are exempt entirely: `mcp__<server>__<tool>` names something a server this build
/// has never spoken to provides, so there is nothing here to check it against and warning would
/// be noise on every project with an MCP server.
pub const KNOWN_TOOLS: &[&str] = &[
    "Agent",
    "Bash",
    "BashOutput",
    "Edit",
    "ExitPlanMode",
    "Glob",
    "Grep",
    "KillShell",
    "NotebookEdit",
    "Read",
    "Task",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
    "Write",
];

/// The keys the front matter may carry.
///
/// Exactly the nine the panel, the parser and the spawn between them consume. Listed so an
/// unknown key can be *named* in its warning together with what was expected, which is the
/// difference between "unknown key `permission_mode`" and a user staring at an underscore.
pub const KNOWN_KEYS: &[&str] = &[
    "name",
    "label",
    "harness",
    "description",
    "model",
    "effort",
    "tools",
    "permission-mode",
    "max-concurrent",
];

// ==========================================================================================
// The front-matter grammar.
// ==========================================================================================

/// The hand-written reader for the fenced block at the top of a definition file.
///
/// The grammar, in full, because it is small enough to state and a user is entitled to know
/// exactly what cide will read:
///
/// ```text
/// line 1            ---                     (exactly, nothing else on it)
/// then, until       key: value              (key is [a-z][a-z0-9_-]*)
/// the closing       # anything              (a whole-line comment, ignored)
/// fence             <blank>                 (ignored)
/// fence             ---
/// then              the body, verbatim, trimmed
/// ```
///
/// Values are scalars. `"quoted"` and `'quoted'` have their outer quotes stripped and are then
/// taken **literally**, with no escape processing — which is the only way to write a value that
/// begins with a character the refusals below reject. Lists are comma- or space-separated, with
/// an optional surrounding `[ ]`.
///
/// Refused, each with its own sentence and line number: a leading space or tab (nesting), a
/// leading `- ` (block sequence), and an unquoted value beginning with `&` (anchor), `*` (alias),
/// `!` (tag), `|` or `>` (block scalar) or `{` (flow mapping). Two deliberate laxities against
/// real YAML: `key:value` with no space is accepted, because the author plainly meant a key and
/// refusing it would be pedantry; and a `#` **inside** a value is part of the value, because
/// `description: fixes issue #42` is far likelier than a trailing comment, and a whole-line
/// comment is available for the other case.
pub mod frontmatter {
    /// One `key: value` line.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Field {
        pub key: String,
        pub value: String,
        /// 1-based, so it can be handed to an editor without arithmetic.
        pub line: u32,
    }

    /// A parsed definition file, before anything has been validated.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Document {
        pub fields: Vec<Field>,
        /// Everything after the closing fence, CRLF-normalised and trimmed.
        ///
        /// Trimmed because the blank line an author leaves after the fence is not part of their
        /// prompt, and because "is this prompt empty" has to be asked of the trimmed text or a
        /// file that is fence-fence-newline reads as a one-character prompt.
        pub body: String,
    }

    /// A refusal, with the line it happened on.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FrontMatterError {
        pub line: u32,
        pub message: String,
    }

    impl FrontMatterError {
        fn at(line: u32, message: impl Into<String>) -> Self {
            Self {
                line,
                message: message.into(),
            }
        }
    }

    /// The fence, and the only spelling of it. `...` closes a YAML document too; it is not
    /// accepted here, because one spelling is one thing to explain in an error message.
    const FENCE: &str = "---";

    /// Read a definition file's front matter and body.
    ///
    /// Never panics and never loops: one pass over the lines, no lookahead, no state beyond
    /// "before the fence / inside / after".
    pub fn parse(text: &str) -> Result<Document, FrontMatterError> {
        // A byte-order mark is what a Windows editor leaves in front of the first `-`, and
        // without this the file fails with "does not begin with ---" while looking, in every
        // editor the user has, exactly as though it does.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);

        let lines: Vec<&str> = text.lines().collect();
        let first = lines.first().map(|l| l.trim_end()).unwrap_or("");
        if first != FENCE {
            return Err(FrontMatterError::at(
                1,
                "a definition must begin with a line containing only `---`, then \
                 `key: value` lines, then `---`, then the system prompt",
            ));
        }

        let mut fields: Vec<Field> = Vec::new();
        for (index, raw) in lines.iter().enumerate().skip(1) {
            // `enumerate` is 0-based and humans are not; every error below is read against an
            // editor's gutter.
            let number = index as u32 + 1;
            let line = raw.trim_end();

            if line.trim().is_empty() {
                continue;
            }
            if line == FENCE {
                // The closing fence. Everything after it is the prompt, verbatim.
                let body = lines[index + 1..].join("\n").trim().to_string();
                return Ok(Document { fields, body });
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                return Err(FrontMatterError::at(
                    number,
                    "indented lines are not supported: cide's front matter is a flat list of \
                     `key: value` lines with no nesting. Write a list as `tools: Read, Edit`.",
                ));
            }
            if line == "-" || line.starts_with("- ") {
                return Err(FrontMatterError::at(
                    number,
                    "`- item` lists are not supported. Write a list on one line, as \
                     `tools: Read, Edit, Write`.",
                ));
            }
            if line.starts_with('#') {
                continue;
            }

            let Some((key, rest)) = line.split_once(':') else {
                // Nearly always the missing closing fence: with no `---` the author's prompt is
                // still being read as front matter, and this is its first prose line. Naming
                // only the grammar would send them to a line that is perfectly fine.
                return Err(FrontMatterError::at(
                    number,
                    format!(
                        "expected `key: value`, found `{line}`. If the front matter ended above \
                         this line, add the closing `---` before the system prompt."
                    ),
                ));
            };
            let key = key.trim();
            if key.is_empty() || !is_key(key) {
                return Err(FrontMatterError::at(
                    number,
                    format!(
                        "`{key}` is not a key: keys are lowercase words joined by `-`, such as \
                         `permission-mode`"
                    ),
                ));
            }
            if let Some(earlier) = fields.iter().find(|f| f.key == key) {
                return Err(FrontMatterError::at(
                    number,
                    format!(
                        "`{key}` is set twice, here and on line {}. Which one wins is not \
                         something cide should be guessing.",
                        earlier.line
                    ),
                ));
            }

            let (value, quoted) = unquote(rest.trim());
            // Only unquoted values are inspected: quoting is the escape hatch, and a user who
            // wrote `description: "*emphasis*"` meant the asterisks.
            if !quoted && let Some(message) = unsupported_scalar(value) {
                return Err(FrontMatterError::at(number, message));
            }
            fields.push(Field {
                key: key.to_string(),
                value: value.to_string(),
                line: number,
            });
        }

        // Falling out of the loop means the fence never closed. The line named is the last one,
        // because that is where the missing `---` goes.
        Err(FrontMatterError::at(
            lines.len().max(1) as u32,
            "the front matter is never closed: add a line containing only `---` between the \
             last key and the system prompt",
        ))
    }

    /// `[a-z][a-z0-9_-]*`. Deliberately narrower than the value grammar: a key is cide's
    /// vocabulary, not the user's, so there is nothing to be liberal about.
    ///
    /// `_` is admitted here although **no key cide reads contains one**, and that is the point:
    /// `permission_mode` is by a distance the likeliest typo in this file, and a tokeniser that
    /// refused the character would report it as "not a key" — a sentence about grammar, from
    /// which the reader has to work out that cide spells it with a hyphen. Letting it through
    /// gets it to the schema layer in `read_definition`, which recognises it as a near-miss of a
    /// key it knows and says *which* key was meant.
    fn is_key(key: &str) -> bool {
        let mut chars = key.chars();
        chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    }

    /// Strip one matching pair of outer quotes, reporting whether there was one.
    ///
    /// The flag is what makes quoting *mean* something: a quoted value skips the refusals below,
    /// so `description: "*not an alias*"` is writable. Without that, a user with a legitimate
    /// leading `*` would have no escape hatch at all and would be told to rewrite their prose.
    fn unquote(value: &str) -> (&str, bool) {
        for quote in ['"', '\''] {
            if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
                return (&value[1..value.len() - 1], true);
            }
        }
        (value, false)
    }

    /// Name the YAML construct this parser will not read, or `None` if the value is an ordinary
    /// scalar.
    ///
    /// Each of these is refused rather than ignored on the module header's argument: a value
    /// silently taken as the literal text `&base` when the author wrote an anchor is a file that
    /// says one thing and an agent that does another.
    fn unsupported_scalar(value: &str) -> Option<String> {
        let first = value.chars().next()?;
        let what = match first {
            '&' => "an anchor (`&name`)",
            '*' => "an alias (`*name`)",
            '!' => "a tag (`!type`)",
            '|' | '>' => "a block scalar (`|` or `>`)",
            '{' => "a flow mapping (`{ … }`)",
            _ => return None,
        };
        Some(format!(
            "{what} is YAML that cide's front matter does not read. Quote the value if you meant \
             it literally: `key: \"{value}\"`."
        ))
    }
}

/// Split a list value: comma- or space-separated, with an optional surrounding `[ ]`.
///
/// Both separators, because both are what people write — `tools: Read, Edit, Write` and
/// `tools: Read Edit Write` are the same list and neither is a mistake. The bracket form is
/// accepted, and *only* the bracket form of YAML's flow collections is: a flow **sequence** is
/// still a flat list of scalars, so reading it costs one `trim_matches`, while a flow mapping is
/// nesting and is refused by name.
fn split_list(value: &str) -> Vec<String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

// ==========================================================================================
// A definition, once parsed.
// ==========================================================================================

/// One role, plus everything about it that `cide_ipc::AgentDef` has nowhere to put.
///
/// # Why this wraps the DTO rather than the DTO growing fields
///
/// `AgentDef` is the *wire* projection: what the roster panel draws. `tools`, `permission-mode`
/// and `effort` are switches only a spawn can act on, and a panel that received them would have
/// to be trusted not to draw them as though they were editable. Keeping them on this side of the
/// wire is the same split `cide_ipc::agents`' own doc describes.
///
/// [`Self::origin`] and [`Self::shadows`] are the exception worth flagging to whoever owns the
/// DTO: the plan wrote them as `AgentDef` fields, the shipped DTO has neither, and they are here
/// instead so that nothing in `cide-ipc` had to change for this slice. They are the answer to
/// *"why is my agent doing that"*, and the panel genuinely wants them — an `origin` field (and a
/// `shadows` beside it) would be a reasonable addition to `AgentDef` when someone is next in
/// there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedAgent {
    /// What crosses the wire, with `unavailable` already decided.
    pub def: AgentDef,
    /// The file this role was read from.
    ///
    /// The single most useful field in the struct for a support question. A role behaving
    /// unexpectedly is nearly always a role read from a file the user forgot they had — most
    /// often the *global* one, in `~/.config/cide/agents/`, edited months ago on a different
    /// project.
    pub origin: PathBuf,
    /// The global definition this project definition replaced, when it replaced one.
    ///
    /// `None` is the ordinary case. `Some` is the failure mode that costs an afternoon: the user
    /// edits `~/.config/cide/agents/developer.md`, sees nothing change, and has no way to
    /// discover that the project's own `.cide/agents/developer.md` has been winning all along.
    /// A shadowed definition must never be silent.
    pub shadows: Option<PathBuf>,
    /// `--allowedTools`, as written. Empty means the definition did not restrict them, which is
    /// the harness's own default and not "no tools".
    pub tools: Vec<String>,
    /// `--permission-mode`, validated against [`PERMISSION_MODES`]. `None` means the harness's
    /// default, which is the safe end of the range.
    pub permission_mode: Option<String>,
    /// A harness-specific reasoning-effort knob, carried verbatim and **not** validated: the set
    /// differs per harness and per release, exactly like [`KNOWN_TOOLS`], and cide has no list to
    /// check it against that would not be wrong within a month.
    pub effort: Option<String>,
}

impl LoadedAgent {
    /// The role's id, which is its name, which is its file stem.
    pub fn id(&self) -> &AgentId {
        &self.def.id
    }

    /// Whether this role may be dispatched at all, ignoring the project's config.
    ///
    /// The config-dependent half — `bypassPermissions` — is `crate::dispatch_refusal`, because it
    /// takes two inputs and this takes one.
    pub fn is_available(&self) -> bool {
        self.def.unavailable.is_none()
    }
}

/// Every role this project has, and everything wrong with the files they came from.
///
/// The two halves are independent on purpose: a catalog with three agents and four problems is
/// the ordinary state of a directory somebody is editing, and neither number tells you the
/// other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Sorted by id, so two windows drawing the same roster draw it in the same order without
    /// either of them agreeing to sort.
    pub agents: Vec<LoadedAgent>,
    pub problems: Vec<AgentProblem>,
}

impl Catalog {
    pub fn get(&self, id: &AgentId) -> Option<&LoadedAgent> {
        self.agents.iter().find(|a| a.def.id == *id)
    }

    /// The findings a user has to act on, as opposed to the ones they may.
    pub fn errors(&self) -> impl Iterator<Item = &AgentProblem> {
        self.problems.iter().filter(|p| p.is_error())
    }
}

// ==========================================================================================
// Names.
// ==========================================================================================

/// `[a-z0-9][a-z0-9-]{0,31}`.
///
/// # This is a path-safety rule before it is a style rule
///
/// An agent name comes from a config file in the user's project, written by whoever — including
/// a model — and it is about to be `Path::join`ed onto `.cide/worktrees/` and turned into the
/// second segment of the ref `cide/<agent>`. A role called `../../etc` would put a checkout
/// wherever it liked; `..` in a ref name is a revision range; a leading `-` is a flag to the next
/// `git` invocation. This is the same class of refusal `cide-app`'s `cmd/file.rs` makes about
/// untrusted paths — check the *shape* first, before anything resolves it — and it is a
/// whitelist rather than a blacklist because a blacklist of `..` and `/` still admits every one
/// of the others, plus a name differing from another only in case, which is one path on macOS
/// and two here (see `check:casing`).
///
/// The identical rule is enforced a second time in `cide_git::worktree::validate_agent`, at the
/// moment the join actually happens. That is not redundancy to tidy away: this crate refuses the
/// name at *load*, so it never reaches the roster, and that crate refuses it at *use*, so a
/// caller that built an `AgentId` some other way cannot get past it either. Neither is allowed
/// to be the only one.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    (1..=32).contains(&name.len())
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `code-reviewer` becomes `Code Reviewer`.
///
/// So that a roster is readable before anybody has thought about presentation, which is what
/// `AgentDef::label`'s doc promises. A `label:` key in the file overrides it, because the
/// title-caser has no idea that `qa` wants to be `QA`.
pub fn label_from_id(id: &str) -> String {
    id.split('-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ==========================================================================================
// One file.
// ==========================================================================================

/// The fields of one definition, after the grammar and before the merge.
///
/// Split out from [`read_definition`] so the field-level validation is one readable pass rather
/// than a `match` arm per key buried inside a loop that is also doing IO.
#[derive(Debug, Default)]
struct Parsed {
    name: Option<(String, u32)>,
    label: Option<String>,
    harness: Option<Harness>,
    description: String,
    model: Option<String>,
    effort: Option<String>,
    tools: Vec<String>,
    permission_mode: Option<String>,
    max_concurrent: Option<u16>,
}

/// A definition that made it far enough to have an identity, and the file it came from.
#[derive(Debug, Clone)]
struct Candidate {
    /// The **declared** name, which is what the role is keyed by.
    ///
    /// Keyed by the declared name and not by the file stem, and that choice is what makes the
    /// duplicate check reachable at all. Stems are unique within a directory, so keying by stem
    /// would make "two files declare one name" impossible to represent — and it is not
    /// impossible, it is the single commonest way this directory goes wrong: `cp developer.md
    /// reviewer.md`, edit the prompt, forget the `name:` line, and now two files claim
    /// `developer` with nothing saying so.
    name: String,
    /// Whether the file's stem agrees with the declared name. The tie-breaker when two files
    /// claim one name: at most one of them can be the file whose own name agrees with its
    /// contents, and that one is unambiguously the intended definition.
    stem_matches: bool,
    agent: LoadedAgent,
}

/// Read and validate one definition file.
///
/// Returns `None` when the file could not produce an identity at all — nothing to list, only
/// problems. Everything else loads, possibly greyed: `AgentDef::unavailable`'s doc is explicit
/// that a role never *vanishes* for being broken, because a role that disappeared when its CLI
/// was uninstalled looks exactly like a role the user deleted.
///
/// The one exception, and it is deliberate: an **invalid name** drops the file entirely. Every
/// other defect leaves something the panel can draw a greyed row for, but the name is the key
/// that a worktree path, a branch ref and every cross-reference are built from, so a name that
/// cannot safely be a directory component cannot safely be an identity either. It is reported as
/// an error against the file, which is where the fix is.
fn read_definition(
    path: &Path,
    text: &str,
    default_harness: Harness,
    problems: &mut Vec<AgentProblem>,
) -> Option<Candidate> {
    let doc = match frontmatter::parse(text) {
        Ok(doc) => doc,
        Err(err) => {
            problems.push(AgentProblem::error(path, Some(err.line), err.message));
            return None;
        }
    };

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let mut parsed = Parsed::default();
    // `unavailable` is decided as the file is read, first reason wins, and the order below is
    // the precedence: an empty prompt beats a bad permission mode beats a stem mismatch. The
    // rest of the reasons are in `problems`, because the row has one line for a sentence and
    // choosing which one it says is better than concatenating three.
    let mut unavailable: Option<String> = None;

    for field in &doc.fields {
        let line = Some(field.line);
        let value = field.value.trim();
        match field.key.as_str() {
            "name" => parsed.name = Some((value.to_string(), field.line)),
            "label" => parsed.label = non_empty(value),
            "description" => parsed.description = value.to_string(),
            "model" => parsed.model = non_empty(value),
            "effort" => parsed.effort = non_empty(value),
            "tools" => {
                parsed.tools = split_list(value);
                for tool in &parsed.tools {
                    // An MCP tool is `mcp__<server>__<tool>` and names something a server cide
                    // has never spoken to provides. There is no list to check it against.
                    if tool.contains("__") || KNOWN_TOOLS.contains(&tool.as_str()) {
                        continue;
                    }
                    problems.push(AgentProblem::warning(
                        path,
                        line,
                        format!(
                            "`{tool}` is not a tool this build of cide knows about. That list is \
                             the CLI's and changes between releases, so this is only a warning — \
                             but check the spelling if the agent cannot do its job."
                        ),
                    ));
                }
            }
            "permission-mode" => {
                if PERMISSION_MODES.contains(&value) {
                    parsed.permission_mode = Some(value.to_string());
                } else {
                    problems.push(AgentProblem::error(
                        path,
                        line,
                        format!(
                            "`{value}` is not a permission mode. Known modes are {}.",
                            PERMISSION_MODES.join(", ")
                        ),
                    ));
                    note_first(
                        &mut unavailable,
                        format!(
                            "`permission-mode: {value}` is not a mode cide recognises ({}).",
                            PERMISSION_MODES.join(", ")
                        ),
                    );
                }
            }
            "harness" => match harness_from_str(value) {
                Some(harness) => parsed.harness = Some(harness),
                None => {
                    problems.push(AgentProblem::error(
                        path,
                        line,
                        format!(
                            "`{value}` is not a harness this build has. Known harnesses are \
                             claude, opencode."
                        ),
                    ));
                    // Loaded and greyed rather than dropped, because "cide does not have that
                    // harness" and "that harness is not installed" are the same sentence to the
                    // user and want the same greyed row with a reason on it.
                    note_first(
                        &mut unavailable,
                        format!("`harness: {value}` is not a harness this build of cide has."),
                    );
                }
            },
            "max-concurrent" => match value.parse::<u16>() {
                Ok(0) => problems.push(AgentProblem::error(
                    path,
                    line,
                    "`max-concurrent: 0` would define a role that can never run. Remove the key \
                     or set it to 1.",
                )),
                Ok(n) => parsed.max_concurrent = Some(n),
                Err(_) => problems.push(AgentProblem::error(
                    path,
                    line,
                    format!("`max-concurrent: {value}` is not a whole number."),
                )),
            },
            other => {
                // Two unknown keys, two different answers. A key that is a *near-miss* of one
                // cide reads — `permission_mode` for `permission-mode` — is a typo, and the
                // switch it meant to set is silently not in effect; since that switch may be the
                // one deciding whether an unattended process asks before editing, the role is
                // greyed rather than run under a setting its author did not choose. A key that
                // resembles nothing only warns, because a definition written for a newer cide
                // arriving in a `git pull` must not empty somebody's roster.
                let canonical = other.replace('_', "-");
                match KNOWN_KEYS.iter().find(|known| ***known == *canonical) {
                    Some(known) => {
                        problems.push(AgentProblem::error(
                            path,
                            line,
                            format!(
                                "`{other}` is not a key cide reads; it spells this one \
                                 `{known}`. As written, the setting has no effect."
                            ),
                        ));
                        note_first(
                            &mut unavailable,
                            format!(
                                "`{other}` is not a key cide reads — the spelling is `{known}` — \
                                 so that setting is not in effect."
                            ),
                        );
                    }
                    None => problems.push(AgentProblem::warning(
                        path,
                        line,
                        format!(
                            "`{other}` is not a key cide reads. Known keys are {}.",
                            KNOWN_KEYS.join(", ")
                        ),
                    )),
                }
            }
        }
    }

    // --- the name, which is the identity and therefore the one fatal field ------------------

    let (name, name_line) = match parsed.name {
        Some((name, line)) => (name, line),
        None => {
            problems.push(AgentProblem::error(
                path,
                None,
                format!(
                    "no `name:` in the front matter. It must be `{stem}`, matching the file name."
                ),
            ));
            return None;
        }
    };
    if !valid_name(&name) {
        problems.push(AgentProblem::error(
            path,
            Some(name_line),
            format!(
                "`{name}` is not a usable agent name. A name is 1–32 characters of a–z, 0–9 and \
                 `-`, starting with a letter or digit — because it becomes a directory under \
                 `.cide/worktrees/` and a git branch, so a name containing `/` or `..` would put \
                 a checkout outside the project."
            ),
        ));
        return None;
    }
    if name != stem {
        problems.push(AgentProblem::error(
            path,
            Some(name_line),
            format!(
                "this file declares `name: {name}` but is called `{stem}.md`. Rename the file to \
                 `{name}.md`, or change the key to `{stem}` — as it stands a copy of another \
                 definition silently claims that definition's name."
            ),
        ));
        note_first(
            &mut unavailable,
            format!("`name: {name}` does not match the file name `{stem}.md`."),
        );
    }

    // --- the prompt, which is the whole point of the file ------------------------------------

    if doc.body.is_empty() {
        problems.push(AgentProblem::error(
            path,
            None,
            "this definition has no system prompt. Everything after the closing `---` is the \
             prompt, and without one the role is the CLI's default agent wearing a label.",
        ));
        note_first(
            &mut unavailable,
            "the definition has no system prompt, so it would run as the harness's default \
             agent under a role's name."
                .to_string(),
        );
    }

    if parsed.description.is_empty() {
        problems.push(AgentProblem::warning(
            path,
            None,
            "no `description:`. The orchestrator is handed this line verbatim when it asks what \
             roles it has, so without one it has only the name to go on.",
        ));
    }

    let id = AgentId(name.clone());
    let label = parsed.label.unwrap_or_else(|| label_from_id(&name));
    Some(Candidate {
        stem_matches: name == stem,
        name,
        agent: LoadedAgent {
            def: AgentDef {
                id,
                label,
                harness: parsed.harness.unwrap_or(default_harness),
                description: parsed.description,
                system_prompt: doc.body,
                model: parsed.model,
                unavailable,
                // 1 rather than 0-means-unlimited: worktree isolation pins a role to one run at
                // a time anyway, and a default a user cannot regret beats one that starts two
                // `claude` processes the first time somebody presses the button.
                max_concurrent: parsed.max_concurrent.unwrap_or(1),
            },
            origin: path.to_path_buf(),
            shadows: None,
            tools: parsed.tools,
            permission_mode: parsed.permission_mode,
            effort: parsed.effort,
        },
    })
}

/// Record a reason for `AgentDef::unavailable` unless one is already there.
///
/// First reason wins, and the call order in [`read_definition`] is the precedence. One `Option`
/// rather than a list because the row has one line for a sentence, and choosing which of three
/// reasons it says is better than concatenating them into something nobody reads to the end —
/// the rest are all in `problems`, against their own lines.
fn note_first(slot: &mut Option<String>, reason: String) {
    if slot.is_none() {
        *slot = Some(reason);
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// The spelling of a harness in a definition file, which is its `serde` name.
///
/// A hand-written match rather than `serde_json::from_value`, so that the *unknown* case is a
/// value this module can put in a sentence rather than a serde error mentioning a variant list
/// in Rust's syntax.
pub fn harness_from_str(value: &str) -> Option<Harness> {
    match value {
        "claude" => Some(Harness::Claude),
        "opencode" => Some(Harness::Opencode),
        _ => None,
    }
}

/// The spelling of a harness on the wire and on disk. The inverse of [`harness_from_str`].
pub fn harness_name(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "claude",
        Harness::Opencode => "opencode",
    }
}

// ==========================================================================================
// Can this build run this role? Two questions, not one.
// ==========================================================================================
//
// `AgentDef::unavailable` used to be set from [`installed`] alone, which asks only *is the
// binary on PATH*. That answered the wrong question for a harness cide has not implemented yet:
// on any machine that happens to have `opencode` — an ordinary tool to have — a role saying
// `harness: opencode` loaded with `unavailable: None`, so the panel drew a working Dispatch
// button, `crate::dispatch_refusal` passed it, the queue gave it a slot, `cide_git::worktree`
// created a **branch and a checkout in the user's repository** for it, and only then did
// `start_child`'s `for_kind` lookup come back empty — from a layer whose vocabulary is spawning,
// with no way to say what had actually gone wrong or to undo what it had already done.
//
// So the two facts are asked separately, because only one of them is the user's to act on:
//
// * [`implemented`] — *does this build of cide have this harness*. A fact about cide, knowable
//   with certainty, and permanent until a release changes it. Nothing the user can do.
// * [`installed`] — *is its binary on this machine*. A fact about the machine, and an
//   instruction: put it on `PATH`.
//
// [`load_from`] asks them in that order. See the comment at the call site for why.

/// The binary each harness is spawned as, when nothing has configured one.
///
/// A bare name on purpose, resolved at each spawn: both of these CLIs update themselves
/// underneath a running app, so pinning an absolute path at load would keep runs on a version
/// that has been replaced. `cide_core::claude_cli::resolve`'s doc makes the same argument at
/// length for the Claude pane.
pub fn harness_binary(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "claude",
        Harness::Opencode => "opencode",
    }
}

/// The default probe: is this harness's binary on the app's `PATH`?
///
/// **Only half of `AgentDef::unavailable`** — see the section banner above and [`implemented`],
/// which is asked first and which this deliberately does not fold in: a probe is a parameter of
/// [`load_from`] precisely so a caller can substitute one, and a substituted probe must not be
/// able to drop the build's own answer on the floor.
///
/// `cide_core::toolchain::which` and not `PATH` alone, because a cide started from a desktop
/// launcher has a different `PATH` from one started in a terminal — that search adds
/// `~/.cargo/bin`, `~/go/bin` and the rest of `toolchain::extra_dirs`, which is the same list
/// `child_env::prepare_command` appends to what the child gets. Probing a narrower `PATH` than
/// the child will actually have would grey a role that would in fact have started.
///
/// Returns the sentence for `AgentDef::unavailable`, or `None` when the binary is there. Note
/// what this does **not** claim: that the binary works, that it is the right version, or that it
/// is not a `rustup`-style shim that fails at exec. `claude_cli::BinaryProblem`'s doc records why
/// that distinction is kept — the probe is a fast, honest "we could not find it", not a promise.
pub fn installed(harness: Harness) -> Option<String> {
    let binary = harness_binary(harness);
    if cide_core::toolchain::which(binary).is_some() {
        return None;
    }
    Some(format!(
        "“{binary}” is not on this app's PATH, nor in ~/.cargo/bin or ~/go/bin. A cide started \
         from a desktop launcher has a different PATH from one started in a terminal."
    ))
}

/// Does this build of cide have an implementation for this harness at all?
///
/// Returns the sentence for `AgentDef::unavailable`, or `None` when it does.
///
/// # Asked of the harness layer, never matched on here
///
/// [`crate::harness::for_kind`] *is* the question — it is the same lookup `start_child` makes at
/// the moment of the fork, so this cannot disagree with it. A `match` on [`Harness`] in this file
/// would be a second place to update when `OpencodeHarness` lands, and the failure mode of
/// forgetting it is the worse direction: a working harness greyed with a sentence saying cide
/// cannot run it. Adding a harness stays what the trait was chosen for — one `static` and one
/// registry entry — and this function follows it for free.
///
/// The sentence names the build rather than the machine, deliberately. “The binary is not on
/// your PATH” is an instruction; this is not, and telling a user to install `opencode` when the
/// build could not use it if they did sends them to fix something that is already fine.
pub fn implemented(harness: Harness) -> Option<String> {
    if crate::harness::for_kind(harness).is_some() {
        return None;
    }
    Some(unimplemented_sentence(harness))
}

/// What [`implemented`] answers with, split out so it can be read without a harness to fail on.
///
/// **Every variant of [`Harness`] currently has an implementation**, so nothing in a running cide
/// reaches this. That is not a reason to delete it and it is exactly the reason to keep it
/// separable: the state it describes — a variant that landed in `cide-ipc` a slice before its
/// `impl Harness` did — is the ordinary shape of a staged delivery, and the sentence has to be
/// right *on the release that first needs it*. A sentence nobody can reach is a sentence nobody
/// proof-reads, so `the_build_and_the_machine_get_different_sentences` reads this one directly.
fn unimplemented_sentence(harness: Harness) -> String {
    format!(
        "this build of cide cannot run “{}” roles: it has no implementation for that harness \
         yet, so there is nothing for it to start whether or not “{}” is installed. Nothing \
         here is yours to fix — it arrives in a release. Until then, give this role \
         `harness: claude`.",
        harness_name(harness),
        harness_binary(harness)
    )
}

// ==========================================================================================
// One directory, then both.
// ==========================================================================================

/// Where a project keeps its own role definitions.
pub fn project_dir(project_root: &Path) -> PathBuf {
    crate::config::cide_dir(project_root).join("agents")
}

/// Where the user's own role definitions live, shared across every project.
///
/// `cide_core::persist::config_dir()`, which is where `keymap.json` already is: these are
/// hand-authored files the user would want in a dotfiles repository, which is exactly the test
/// that module's doc uses to tell config from state.
pub fn global_dir() -> PathBuf {
    cide_core::persist::config_dir().join("agents")
}

/// Read every `*.md` in one directory.
///
/// A missing directory is **not** a problem and produces nothing: no `.cide/agents/` is the
/// state of every project that has never used this feature, and reporting it would put a
/// permanent finding on every repository in the world. A directory that exists and cannot be
/// read is a warning, because that one is genuinely unexpected.
///
/// Entries are sorted by file name before anything is parsed, so the duplicate tie-break below
/// and the resulting order are the same on every machine — `read_dir` order is the filesystem's
/// and is not stable between two checkouts of the same repository.
fn load_scope(
    dir: &Path,
    default_harness: Harness,
    problems: &mut Vec<AgentProblem>,
) -> BTreeMap<String, Candidate> {
    let mut found: BTreeMap<String, Candidate> = BTreeMap::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return found,
        Err(err) => {
            problems.push(AgentProblem::warning(
                dir,
                None,
                format!("this directory could not be read: {err}"),
            ));
            return found;
        }
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    paths.sort();

    for path in paths {
        // Not `is_file()` first: a definition that is a symlink into a dotfiles repository is a
        // reasonable thing to have, and `read_to_string` follows it. A directory called
        // `foo.md` fails the read and is reported, which is the right amount of attention.
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                problems.push(AgentProblem::error(
                    &path,
                    None,
                    format!("this file could not be read: {err}"),
                ));
                continue;
            }
        };
        let Some(candidate) = read_definition(&path, &text, default_harness, problems) else {
            continue;
        };

        // Two files in one directory claiming one name. The winner is the file whose own name
        // agrees with its `name:` key, because at most one file can be that and it is
        // unambiguously the intended definition — which leaves the realistic case (`cp
        // developer.md developer-old.md`) with the original still working and the copy flagged.
        // When *neither* file's stem matches, nothing distinguishes them, so the survivor is
        // greyed rather than picked: guessing which of two identically-named roles the user
        // meant is precisely the "an agent nobody wrote" failure this crate exists to avoid.
        if let Some(existing) = found.get(&candidate.name) {
            problems.push(AgentProblem::error(
                &candidate.agent.origin,
                None,
                format!(
                    "`{}` is also declared by `{}`. One name is one role; rename or delete one \
                     of the two files.",
                    candidate.name,
                    existing.agent.origin.display()
                ),
            ));
            let both = format!(
                "declared twice, by `{}` and `{}`.",
                existing.agent.origin.display(),
                candidate.agent.origin.display()
            );
            let mut winner = if candidate.stem_matches && !existing.stem_matches {
                candidate
            } else {
                existing.clone()
            };
            // Overriding whatever file-level reason the winner already carried, deliberately.
            // A scope-level ambiguity outranks a defect in one file, because until it is
            // resolved the user cannot even tell *which* file's defect they are being shown —
            // and this sentence names both paths, which is the fix.
            if !winner.stem_matches {
                winner.agent.def.unavailable = Some(both);
            }
            found.insert(winner.name.clone(), winner);
            continue;
        }
        found.insert(candidate.name.clone(), candidate);
    }

    found
}

/// Read the global and project directories and merge them.
///
/// # Whole-file override, not a field-by-field merge
///
/// A project definition replaces a global one of the same name **entirely**. The tempting
/// alternative — take the global prompt, let the project override `tools` — produces an agent
/// nobody wrote and nobody reviewed: a definition is a system prompt *plus the switches that
/// make that prompt safe*, and the two halves were authored together. A prompt that says "you
/// may edit anything under src/" is safe next to the `tools` line its author wrote and is not
/// safe next to somebody else's. Whole-file override is also the rule `keymap.json` already uses
/// for a binding, so there is one merge rule in cide rather than two.
///
/// `probe` answers *is this harness's binary on the machine*. It is a parameter rather than a
/// call to [`installed`] so that tests need no `claude` on `PATH`, and so the later dispatch
/// slice can probe the user's *configured* binary from `Settings` instead of the default name.
/// It is only ever consulted for a harness this build can actually run — see [`implemented`] and
/// the loop below.
pub fn load_from(
    global: &Path,
    project: &Path,
    default_harness: Harness,
    probe: impl Fn(Harness) -> Option<String>,
) -> Catalog {
    let mut problems = Vec::new();
    let globals = load_scope(global, default_harness, &mut problems);
    let projects = load_scope(project, default_harness, &mut problems);

    let mut merged = globals;
    for (name, mut candidate) in projects {
        if let Some(shadowed) = merged.get(&name) {
            candidate.agent.shadows = Some(shadowed.agent.origin.clone());
        }
        merged.insert(name, candidate);
    }

    // One probe per distinct harness, not one per role. `which` walks every `PATH` entry with a
    // `stat` per candidate, and this runs on every roster read — which happens on every
    // `.cide/` filesystem event.
    let mut probed: Vec<(Harness, Option<String>)> = Vec::new();
    let mut agents: Vec<LoadedAgent> = Vec::new();
    for candidate in merged.into_values() {
        let mut agent = candidate.agent;
        if agent.def.unavailable.is_none() {
            let harness = agent.def.harness;
            // **`implemented` first, and `probe` only if it passes.** A role whose harness this
            // build cannot run is unavailable *whatever* is on `PATH`, so asking the machine
            // afterwards could only ever produce the wrong sentence: on a machine that does have
            // `opencode` the probe says nothing and the role comes back dispatchable, which is
            // exactly the bug the section banner describes; on one that does not, the user is
            // told to install a program that would not help. The build's answer is also the
            // certain one — a `for_kind` lookup, not a filesystem guess — and certainty outranks
            // a probe whose own doc says it is a fast "we could not find it" rather than a
            // promise. It is deliberately *not* folded into `probe`: `probe` is a parameter, and
            // a substituted one must not be able to drop this answer.
            //
            // Both lose to a fault in the file itself, which is the `is_none()` guard above and
            // is unchanged: "your `permission-mode` is misspelt" is the sentence the user can
            // act on today, and it outranks both facts about the world.
            let reason =
                implemented(harness).or_else(|| match probed.iter().find(|(h, _)| *h == harness) {
                    Some((_, reason)) => reason.clone(),
                    None => {
                        let reason = probe(harness);
                        probed.push((harness, reason.clone()));
                        reason
                    }
                });
            agent.def.unavailable = reason;
        }
        agents.push(agent);
    }
    // `BTreeMap::into_values` already yields name order and `id` *is* the name, so this is a
    // no-op today. It is here so that a later change to the map type cannot silently make two
    // windows draw the roster in two orders.
    agents.sort_by(|a, b| a.def.id.cmp(&b.def.id));

    Catalog { agents, problems }
}

/// [`load_from`] against the real directories and the real `PATH`.
///
/// `default_harness` comes from `.cide/config.json`; see `crate::config::AgentsConfig::harness`
/// for why a project gets to choose it and a definition gets to override it.
pub fn load(project_root: &Path, default_harness: Harness) -> Catalog {
    load_from(
        &global_dir(),
        &project_dir(project_root),
        default_harness,
        installed,
    )
}

// ==========================================================================================
// Writing one.
// ==========================================================================================
//
// Everything above this line reads. This half writes, and it exists because until now the only
// way to define a role was to open an editor and get the front matter right — which is a fair
// thing to ask of whoever designed the format and of nobody else.
//
// # The invariant the whole section is built to hold
//
// **`parse_draft(&render(d)) == normalize(d)`, for every draft there is.** A form that cannot
// read back what it wrote is a form that quietly drops a field the first time somebody types a
// quote into it, and the file it drops the field from is committed. [`normalize`] is the fixed
// point rather than `d` itself because the grammar has one — a value's surrounding whitespace is
// trimmed by the parser, a body's is too, and CRLF is folded to LF by `str::lines` — so a draft
// that differs from its own normal form is a draft no file can represent. `normalize` is
// idempotent and [`render`] applies it, so a saved definition is always at that fixed point and
// the invariant holds unconditionally from the second save onwards.
//
// # How `render` stops a body from closing its own front matter
//
// It does not need to escape the body at all, and that is the answer rather than an omission.
// [`frontmatter::parse`] scans forward from line 2 and **stops at the first line that is exactly
// `---`**; everything after that line is taken verbatim and is never looked at again. `render`
// emits that fence itself, immediately after the last key, so by the time the body starts the
// scanner has already returned. A `---` in the prompt, a `model: sonnet` as its first line, a
// stray `# comment`, a `- item` — none of them are grammar any more, because nothing is reading
// them as grammar.
//
// What could break that is not the body. It is a **value** containing a line break: one `\n` in
// a `description` and the rest of that description becomes lines inside the front-matter region,
// where `---` *is* the fence and `key:` *is* a key. So the one thing `render` must guarantee is
// that every value it emits occupies exactly one line, and it guarantees it by running every
// draft through [`normalize`] first — which folds `\r` and `\n` in a scalar to a space — rather
// than by trusting a caller to have validated. There is one normalizer, `render` applies it, and
// a value therefore cannot open a line it was not given.
//
// The second hazard is smaller and is the parser's own escape hatch used in reverse: an unquoted
// value may not *begin* with `&`, `*`, `!`, `|`, `>` or `{`, because each of those opens a YAML
// construct this parser refuses by name. [`needs_quotes`] wraps those, and wraps one more case
// the refusals do not cover — a value that already looks quoted, whose outer pair `unquote`
// would eat. Quoting is a plain pair of `"` with **no escaping**, which is exactly right here:
// `unquote` strips one outer pair and takes what is left literally, so `"` and `\` inside a
// value survive untouched and there is no escape syntax for either side to disagree about.

/// What went wrong writing a definition.
///
/// [`Self::Rejected`] is the user's form to fix and everything else is not, which is the split
/// `cmd::agents` maps onto the wire: a rejection is a described outcome with a next action in it,
/// the rest are failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The draft is not writable as it stands, with a reason per field. **Nothing was written.**
    Rejected(Vec<AgentDraftProblem>),
    /// There is no definition file there. Answered rather than treated as success, because a
    /// delete that silently succeeds on a file that was not there hides the mistake it is
    /// likeliest to be — acting on the project scope while looking at the global definition that
    /// the project one was shadowing.
    NotFound(PathBuf),
    /// The file exists and its front matter does not parse, so there is no draft to populate a
    /// form with. Carries the line, because that is the whole reason the parser is hand-written.
    Unreadable {
        path: PathBuf,
        line: u32,
        message: String,
    },
    Io {
        path: PathBuf,
        error: String,
    },
    /// A rename wrote the new file and could not remove the old one.
    ///
    /// Its own variant rather than an [`Self::Io`], because the caller's situation is different
    /// in a way a generic IO sentence cannot convey: the save **worked**, and the project now has
    /// two files claiming one name, which is a state [`load_scope`] already reports and greys.
    /// Told plainly here so the user reads it as "finish the move" rather than as "the save
    /// failed", which would send them to press the button again.
    Stranded {
        wrote: PathBuf,
        left: PathBuf,
        error: String,
    },
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(problems) => {
                // Joined only for the `Display` path, which is a log line and an `Err` of last
                // resort. The structured list is what the form reads; see `AgentDraftProblem`.
                let joined: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
                write!(f, "{}", joined.join(" "))
            }
            Self::NotFound(path) => write!(
                f,
                "there is no definition at {} — nothing was changed.",
                path.display()
            ),
            Self::Unreadable {
                path,
                line,
                message,
            } => write!(f, "{}:{line}: {message}", path.display()),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Stranded { wrote, left, error } => write!(
                f,
                "the definition was written to {}, but the file it replaced could not be \
                 removed ({}: {error}). Both files now declare this role and cide will grey it \
                 until one of them is gone; deleting {} finishes the move.",
                wrote.display(),
                left.display(),
                left.display()
            ),
        }
    }
}

impl std::error::Error for WriteError {}

/// The directory one scope's definitions live in.
///
/// `project_root` is ignored for [`AgentScope::Global`], deliberately and visibly: the global
/// scope is *not* per project, and a signature that took the root only for the project arm would
/// make two functions out of one concept and let a caller reach the global directory without
/// having thought about which scope it wanted.
pub fn scope_dir(project_root: &Path, scope: AgentScope) -> PathBuf {
    match scope {
        AgentScope::Project => project_dir(project_root),
        AgentScope::Global => global_dir(),
    }
}

/// The file one role occupies, or `None` when the name may not become a path component.
///
/// **The only place in this half of the module that joins a name onto a directory**, and the
/// `Option` is the reason it is safe: [`valid_name`]'s doc explains what a name that reaches
/// `Path::join` unchecked can do — `../../etc` puts a definition wherever it likes — and this
/// answers `None` rather than trusting that somebody upstream asked. [`save`], [`delete`] and
/// [`read_draft`] all go through it, so there is one join to audit rather than three.
pub fn definition_path(project_root: &Path, scope: AgentScope, name: &str) -> Option<PathBuf> {
    valid_name(name).then(|| scope_dir(project_root, scope).join(format!("{name}.md")))
}

/// A one-line value, as the grammar can hold it: no line breaks, no surrounding whitespace.
///
/// The line-break fold is the fence's whole defence — see the section header. The trim is the
/// parser's own behaviour reflected back: `parse` trims a value and `read_definition` trims it
/// again, so a value with spaces around it is one no file can round-trip.
fn scalar(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

/// A body, as the grammar can hold it: LF line endings, no surrounding blank space.
///
/// `str::lines` folds CRLF for free on the way in, and `parse` trims the joined result, so both
/// are applied here on the way out — otherwise a prompt pasted from a Windows editor would come
/// back different from what was saved and the form would report a change nobody made.
fn body(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

/// The draft, in the only shape a definition file can hold.
///
/// Idempotent — `normalize(normalize(d)) == normalize(d)` — which is what makes it usable as the
/// right-hand side of the round-trip invariant, and it is asserted rather than assumed.
///
/// Empty optionals collapse to `None`, matching [`non_empty`] on the read side: `label: ""` and
/// no `label:` key at all are the same file, so they must be the same draft or a save-then-reload
/// would show the user a field changing on its own.
///
/// [`AgentDraft::original`] passes through untouched. It is not a key in the file and has no
/// normal form here; it is the form's memory of which file it opened.
pub fn normalize(draft: &AgentDraft) -> AgentDraft {
    let optional = |value: &Option<String>| {
        value
            .as_deref()
            .map(scalar)
            .filter(|value| !value.is_empty())
    };
    AgentDraft {
        scope: draft.scope,
        name: AgentId(scalar(draft.name.as_str())),
        original: draft.original.clone(),
        label: optional(&draft.label),
        harness: draft.harness,
        description: scalar(&draft.description),
        model: optional(&draft.model),
        effort: optional(&draft.effort),
        tools: draft
            .tools
            .iter()
            .map(|tool| scalar(tool))
            .filter(|tool| !tool.is_empty())
            .collect(),
        permission_mode: optional(&draft.permission_mode),
        max_concurrent: draft.max_concurrent,
        system_prompt: body(&draft.system_prompt),
    }
}

/// A draft as the whole text of its `.md` file.
///
/// Keys are emitted in [`KNOWN_KEYS`] order and **only** keys in that list are emitted, which is
/// the promise that makes a rendered file readable by the parser above with nothing left over.
/// A key whose value is absent or empty is omitted rather than written blank: `label:` with
/// nothing after it and no `label:` line mean the same thing to the reader, and the shorter file
/// is the one a person reviewing a pull request can take in.
///
/// Nothing here can fail. Rendering an invalid draft produces a file that
/// [`read_definition`] will complain about in exactly the way it complains about a hand-written
/// one — which is why [`save`] validates first, and why `render` itself is total: a `Result` on
/// this function would be a second, weaker copy of [`validate`] that every caller would have to
/// handle and none could act on.
pub fn render(draft: &AgentDraft) -> String {
    // Normalized here rather than at the call sites, so that `render` is the guarantee and not a
    // function with a precondition. See the section header: a value that spans two lines is the
    // one thing that could put the body inside the front matter.
    let draft = normalize(draft);

    let mut out = String::from("---\n");
    field(&mut out, "name", draft.name.as_str());
    if let Some(label) = &draft.label {
        field(&mut out, "label", label);
    }
    if let Some(harness) = draft.harness {
        field(&mut out, "harness", harness_name(harness));
    }
    if !draft.description.is_empty() {
        field(&mut out, "description", &draft.description);
    }
    if let Some(model) = &draft.model {
        field(&mut out, "model", model);
    }
    if let Some(effort) = &draft.effort {
        field(&mut out, "effort", effort);
    }
    if !draft.tools.is_empty() {
        // `, ` because both separators read back the same and this one survives a `git diff`
        // legibly. `split_list` accepts spaces too, and a comma is what people write.
        field(&mut out, "tools", &draft.tools.join(", "));
    }
    if let Some(mode) = &draft.permission_mode {
        field(&mut out, "permission-mode", mode);
    }
    if let Some(max) = draft.max_concurrent {
        field(&mut out, "max-concurrent", &max.to_string());
    }
    out.push_str("---\n\n");

    // The blank line above and the newline below are cosmetic — `parse` trims the body at both
    // ends — and they are here because this file is read and reviewed by people, and a prompt
    // welded to its own closing fence reads as part of it.
    out.push_str(&draft.system_prompt);
    out.push('\n');
    out
}

fn field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    if needs_quotes(value) {
        out.push('"');
        out.push_str(value);
        out.push('"');
    } else {
        out.push_str(value);
    }
    out.push('\n');
}

/// Would this value be read as something other than itself if written bare?
///
/// Two cases, and both are `frontmatter`'s own rules read backwards.
///
/// * A leading `&`, `*`, `!`, `|`, `>` or `{` is a YAML construct `unsupported_scalar` refuses by
///   name, and the refusal's own advice is to quote it — so a description that legitimately
///   begins with `*emphasis*` is written `description: "*emphasis*"` and comes back with its
///   asterisks.
/// * A value that *is* a matching pair of quotes would have that pair eaten by `unquote`, which
///   strips one outer pair before anything else looks at the value. `'hello'` written bare reads
///   back as `hello`; written as `"'hello'"` it reads back as itself.
///
/// The wrapping needs no escaping at all, which is worth stating because it looks too easy:
/// `unquote` removes exactly one leading and one trailing byte and takes the remainder
/// **literally**, so a value full of quotes and backslashes survives a `"` pair unchanged. There
/// is no escape grammar on either side, therefore no escape grammar for the two sides to
/// disagree about — which is the failure this half of the module would otherwise be most likely
/// to have.
fn needs_quotes(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    if matches!(first, '&' | '*' | '!' | '|' | '>' | '{') {
        return true;
    }
    // Byte length, to match `unquote`'s own `value.len() >= 2` exactly: a one-character `"` is
    // not a pair there and must not be treated as one here.
    value.len() >= 2 && matches!(first, '"' | '\'') && value.ends_with(first)
}

/// The inverse of [`render`]: a file's text as the draft a form edits.
///
/// **Not [`read_definition`], and the difference is the point.** That function answers *what will
/// this role do*, so it resolves every absence into a default — a missing `harness:` becomes the
/// project's, a missing `label:` becomes the title-cased name, a missing `max-concurrent:`
/// becomes 1. A form must show what the **file says**, because a form is the thing that writes it
/// back: populated from resolved values it would write `harness: claude` into a definition that
/// deliberately followed the project default, and freeze it there.
///
/// `original` is `None` on the way out. A file's text does not record which file it is; that is
/// [`read_draft`]'s to stamp, and keeping it out of here is what lets the round-trip invariant be
/// stated over the text alone.
///
/// # What this drops
///
/// Front-matter comments, unknown keys, and any value cide's own vocabulary cannot hold — a
/// `harness: bogus` or a `max-concurrent: lots` reads as absent, and saving the draft therefore
/// deletes the line. `AgentDraft`'s doc argues why that is the design rather than a defect: the
/// alternative is a draft carrying text the form cannot show and is nonetheless responsible for
/// preserving. Every one of those cases is already an error or a warning on the roster, against
/// its own line, before anybody opens the form.
pub fn parse_draft(
    text: &str,
    scope: AgentScope,
) -> Result<AgentDraft, frontmatter::FrontMatterError> {
    let doc = frontmatter::parse(text)?;
    let value = |key: &str| {
        doc.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| field.value.trim())
    };
    let optional = |key: &str| {
        value(key)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };

    Ok(AgentDraft {
        scope,
        name: AgentId(value("name").unwrap_or_default().to_string()),
        original: None,
        label: optional("label"),
        harness: value("harness").and_then(harness_from_str),
        description: value("description").unwrap_or_default().to_string(),
        model: optional("model"),
        effort: optional("effort"),
        tools: value("tools").map(split_list).unwrap_or_default(),
        permission_mode: optional("permission-mode"),
        max_concurrent: value("max-concurrent").and_then(|value| value.parse::<u16>().ok()),
        system_prompt: doc.body,
    })
}

// ------------------------------------------------------------------------------------------
// Validation, which belongs here and not in the form.
// ------------------------------------------------------------------------------------------

/// Every reason this draft cannot be written, against the field that carries it.
///
/// # Why the rules are here rather than in the form
///
/// Because they are already here. [`valid_name`] is a path-safety rule with a paragraph about
/// what an unchecked name does to `Path::join`, [`PERMISSION_MODES`] is somebody else's
/// vocabulary that this module is the keeper of, and the empty-prompt rule is
/// [`read_definition`]'s. A copy in TypeScript would be a second answer to each — and the copy
/// would drift towards being the *only* one consulted, because it is the one the user sees,
/// while `.cide/agents/` also has a hand editor and an MCP writer that never touch a form.
///
/// # Why every reason, and why each names a field
///
/// A form that surfaces one problem at a time makes the user press Save four times to discover
/// four problems. And a form that says "invalid" without saying where is a form the user cannot
/// fix: the two rules that actually fire — a name that may not be a directory component, a
/// permission mode from a CLI release this build has not heard of — are precisely the ones whose
/// reason is invisible from the box they belong to.
///
/// Warnings are deliberately **not** returned. An unknown tool name and a missing description are
/// findings [`read_definition`] raises against the file and the roster draws; a form that refused
/// them would refuse every MCP tool, which is exactly what [`KNOWN_TOOLS`]' doc says must not
/// happen. This function answers one question — *may this be written* — and a list that mixed the
/// two would be read as a list of blockers.
///
/// Normalizes first, so a caller may hand it raw form state and get the answer [`save`] would
/// give. Nothing here can be fixed by normalizing, which is the test for what belongs in each:
/// a stray newline in a description is repairable and is repaired, a name containing `/` is not.
pub fn validate(draft: &AgentDraft) -> Vec<AgentDraftProblem> {
    let draft = normalize(draft);
    let mut problems = Vec::new();

    if draft.name.as_str().is_empty() {
        problems.push(problem(
            AgentField::Name,
            "a role needs a name. It becomes the file name — `<name>.md` — and the word the \
             orchestrator uses to ask for this role by.",
        ));
    } else if !valid_name(draft.name.as_str()) {
        problems.push(problem(AgentField::Name, invalid_name(draft.name.as_str())));
    }

    if let Some(location) = &draft.original
        && !valid_name(location.name.as_str())
    {
        // Not reachable from a form that was populated by `read_draft` — the roster cannot list
        // a role whose name `read_definition` refused. Checked anyway because this is the value
        // the *removal* half of a rename joins onto a directory, and a guard that only covers
        // the name being written would leave the other join unchecked.
        problems.push(problem(
            AgentField::Name,
            format!(
                "this form says it is editing a role called `{}`, which is not a usable name. \
                 {}",
                location.name,
                invalid_name(location.name.as_str())
            ),
        ));
    }

    for tool in &draft.tools {
        if tool.contains([' ', '\t', ',', '[', ']']) {
            problems.push(problem(
                AgentField::Tools,
                format!(
                    "`{tool}` cannot be one tool name: the list is written on a single line and \
                     separated by commas or spaces, so a name containing either is read as two \
                     names. Tool names look like `Read`, `Bash` or `mcp__server__tool`."
                ),
            ));
        }
    }

    if let Some(mode) = &draft.permission_mode
        && !PERMISSION_MODES.contains(&mode.as_str())
    {
        problems.push(problem(
            AgentField::PermissionMode,
            format!(
                "`{mode}` is not a permission mode. Known modes are {}. This is the CLI's \
                 vocabulary and cide's copy of it can go stale, so if a release has added one, \
                 leave this unset until cide catches up rather than guessing.",
                PERMISSION_MODES.join(", ")
            ),
        ));
    }

    if draft.max_concurrent == Some(0) {
        problems.push(problem(
            AgentField::MaxConcurrent,
            "0 would define a role that can never run. Leave it unset to take the default of 1.",
        ));
    }

    if draft.system_prompt.is_empty() {
        problems.push(problem(
            AgentField::SystemPrompt,
            "a role needs a system prompt. It is the whole of what makes this a role rather than \
             a name: without one the run is the harness's default agent wearing a label, and \
             `read_definition` greys it for exactly that reason.",
        ));
    }

    problems
}

fn problem(field: AgentField, message: impl Into<String>) -> AgentDraftProblem {
    AgentDraftProblem {
        field,
        message: message.into(),
    }
}

/// [`valid_name`]'s refusal as a sentence, shared by the form and by `read_definition`'s
/// wording so that a user who has seen one recognises the other.
fn invalid_name(name: &str) -> String {
    format!(
        "`{name}` is not a usable agent name. A name is 1–32 characters of a–z, 0–9 and `-`, \
         starting with a letter or digit — because it becomes a directory under \
         `.cide/worktrees/` and a git branch, so a name containing `/` or `..` would put a \
         checkout outside the project."
    )
}

// ------------------------------------------------------------------------------------------
// The three acts: read one into a form, write one back, remove one.
// ------------------------------------------------------------------------------------------

/// One definition file, as the draft a form populates from.
///
/// Read fresh at the moment the form opens, and that is the whole reason this is a command of its
/// own rather than a field on the roster: a draft carried on an `AgentRoster` snapshot is as old
/// as the snapshot, and saving it would silently revert whatever the user's editor — or an agent
/// holding this repository — wrote in between.
///
/// Two repairs happen on the way out, and both are the repair the roster's own error message
/// asks for. A file with no `name:` key takes the stem, and a file whose `name:` disagrees with
/// its stem keeps the **declared** name while `original` records the stem — so saving it
/// unchanged renames the file to match its contents, which is precisely what
/// `read_definition`'s "rename the file to `<name>.md`" error tells the user to do.
pub fn read_draft(
    project_root: &Path,
    scope: AgentScope,
    name: &str,
) -> Result<AgentDraft, WriteError> {
    let path = definition_path(project_root, scope, name)
        .ok_or_else(|| WriteError::Rejected(vec![problem(AgentField::Name, invalid_name(name))]))?;

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(WriteError::NotFound(path));
        }
        Err(error) => {
            return Err(WriteError::Io {
                path,
                error: error.to_string(),
            });
        }
    };

    let mut draft = parse_draft(&text, scope).map_err(|error| WriteError::Unreadable {
        path: path.clone(),
        line: error.line,
        message: error.message,
    })?;
    if draft.name.as_str().is_empty() {
        draft.name = AgentId(name.to_string());
    }
    draft.original = Some(AgentLocation {
        scope,
        name: AgentId(name.to_string()),
    });
    Ok(draft)
}

/// Write a draft to the file it names, moving the file it came from when the two differ.
///
/// # A rename is a move, and the order it happens in is the decision
///
/// `name` must equal the file stem — [`read_definition`] refuses a definition where it does not —
/// so changing the name in a form is a **rename of a file**, and changing the scope is a move
/// between two directories. Both are the same operation here: write the new path, then remove the
/// old one. [`AgentDraft::original`] is what says which old one, because nothing in the draft's
/// contents could: a name in the box that differs from the name on disk is a rename if the form
/// was opened on a role and a create if it was not, and those two must not be guessed between.
///
/// **The write comes first, and that ordering is the whole of the crash story.** Remove-then-write
/// leaves a window in which a failure has destroyed a system prompt and produced nothing; the
/// order here leaves a window in which both files exist, which is a state `load_scope` already
/// names ("`x` is also declared by …"), already greys, and a user already fixes by deleting one.
/// Losing a prompt somebody wrote is not recoverable; a duplicate is.
///
/// If the removal itself fails the save still succeeded, and [`WriteError::Stranded`] says so in
/// those words rather than reporting a failure the user would answer by pressing Save again.
///
/// # A name that is already taken is refused, never overwritten
///
/// The file a clobbering save would replace is somebody's system prompt, quite possibly a
/// teammate's and quite possibly not yet read by the person at the keyboard. So a create whose
/// target exists, and a rename onto an occupied name, both come back as
/// [`WriteError::Rejected`] against the field the user changed — `name` when the name moved,
/// `scope` when only the scope did, because that is the box they need to look at.
///
/// Scopes are checked independently: creating a project `developer` while a global one exists is
/// **allowed**, and is the shadowing `load_from` is built around. The refusal is about a file,
/// not about a name being unique across the merge.
///
/// # Why the shared atomic write
///
/// `.cide/agents/*.md` is committed and read by whichever account a teammate, a pair-programming
/// session or a CI job runs as, which is the argument [`cide_tasks::write_shared`] records at
/// length for `.cide/tasks.json`: 0600 there is not privacy, it is a file the team cannot read.
/// `persist::write_atomic_with_mode` is reached for rather than a temp-then-rename written out
/// here, because the mode, the `sync_all`, the directory `fsync` and the temp cleanup are exactly
/// the parts a second copy starts identical to and drifts on.
///
/// The **global** scope gets the same mode although no team reads it, and that is deliberate: a
/// scope change moves one file between the two directories, and a save whose mode depended on
/// where the file landed would silently change the permissions of a file the user is only moving.
/// Both are ceilings the umask can lower, so neither widens anything — see
/// `write_atomic_with_mode`'s own doc.
pub fn save(project_root: &Path, draft: &AgentDraft) -> Result<PathBuf, WriteError> {
    let draft = normalize(draft);
    let problems = validate(&draft);
    if !problems.is_empty() {
        return Err(WriteError::Rejected(problems));
    }

    // `validate` has already refused a name that cannot be a path component, so neither of these
    // can be `None`. Asked again rather than unwrapped because this is where the join happens,
    // and a guard that lives at the join is a guard a later refactor cannot separate from it.
    let target =
        definition_path(project_root, draft.scope, draft.name.as_str()).ok_or_else(|| {
            WriteError::Rejected(vec![problem(
                AgentField::Name,
                invalid_name(draft.name.as_str()),
            )])
        })?;
    let previous = match &draft.original {
        Some(location) => Some(
            definition_path(project_root, location.scope, location.name.as_str()).ok_or_else(
                || {
                    WriteError::Rejected(vec![problem(
                        AgentField::Name,
                        invalid_name(location.name.as_str()),
                    )])
                },
            )?,
        ),
        None => None,
    };

    let moving = previous.as_deref() != Some(target.as_path());
    if moving && target.exists() {
        return Err(WriteError::Rejected(vec![taken(&draft, &target)]));
    }

    cide_core::persist::write_atomic_with_mode(
        &target,
        render(&draft).as_bytes(),
        cide_core::persist::SHARED_MODE,
    )
    .map_err(|error| WriteError::Io {
        path: target.clone(),
        error: error.to_string(),
    })?;

    if moving && let Some(old) = previous {
        match std::fs::remove_file(&old) {
            Ok(()) => {}
            // The form was opened on a file that has since gone — a `git checkout`, the user's
            // own editor, another window. The move is complete either way and there is nothing
            // to tell anybody.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WriteError::Stranded {
                    wrote: target,
                    left: old,
                    error: error.to_string(),
                });
            }
        }
    }

    Ok(target)
}

/// The refusal for a save that would land on a file it did not open.
///
/// Fielded on `scope` when the scope is the only thing that moved, and on `name` otherwise,
/// because a form highlighting the wrong box is a form that sends the user to change the wrong
/// thing — and "there is already a `developer` here" reads as a name problem when the user's
/// name box is untouched and their scope selector is not.
fn taken(draft: &AgentDraft, target: &Path) -> AgentDraftProblem {
    let scope_only = draft
        .original
        .as_ref()
        .is_some_and(|origin| origin.name == draft.name && origin.scope != draft.scope);
    let field = if scope_only {
        AgentField::Scope
    } else {
        AgentField::Name
    };
    problem(
        field,
        format!(
            "{} already exists, and it holds somebody's system prompt — cide will not write over \
             it. Pick another name, or open that definition and edit it.",
            target.display()
        ),
    )
}

/// Remove one definition file. The delete half of the form.
///
/// Scoped, for [`AgentScope`]'s reason: a project definition shadowing a global one draws a
/// single row, and a delete that guessed would take the wrong file — most likely the *global*
/// one, which is shared across every project the user opens and is not in any repository's
/// history to be recovered from.
///
/// A missing file is [`WriteError::NotFound`] rather than a quiet success. Deleting something
/// that was not there is not the harmless no-op it looks like: it is the exact symptom of having
/// aimed at the wrong scope, and a silent success would leave the row on screen with nothing
/// saying why.
///
/// Nothing is written, so there is no atomic-publish story here — but the name still goes through
/// [`definition_path`], because an unchecked join is as dangerous for an `unlink` as it is for a
/// write, and more so.
pub fn delete(project_root: &Path, name: &str, scope: AgentScope) -> Result<PathBuf, WriteError> {
    let path = definition_path(project_root, scope, name)
        .ok_or_else(|| WriteError::Rejected(vec![problem(AgentField::Name, invalid_name(name))]))?;

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(WriteError::NotFound(path))
        }
        Err(error) => Err(WriteError::Io {
            path,
            error: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory, built the way `cide-core`'s tests build theirs.
    ///
    /// `std::env::temp_dir()` and the pid rather than a temp-dir crate, because this workspace
    /// has none and is not gaining one for a test helper. The tag keeps two tests in one binary
    /// apart; the pid keeps two `cargo test` runs apart.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-agents-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(dir: &Path, file: &str, text: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("scope dir");
        let path = dir.join(file);
        std::fs::write(&path, text).expect("definition");
        path
    }

    /// The probe that says every harness is installed, so a test asserts on the parser rather
    /// than on whether the machine running it happens to have `claude`.
    fn present(_: Harness) -> Option<String> {
        None
    }

    fn missing(_: Harness) -> Option<String> {
        Some("“claude” is not on this app's PATH.".to_string())
    }

    fn agent<'a>(catalog: &'a Catalog, name: &str) -> &'a LoadedAgent {
        catalog
            .get(&AgentId(name.to_string()))
            .unwrap_or_else(|| panic!("no `{name}` in {:?}", ids(catalog)))
    }

    fn ids(catalog: &Catalog) -> Vec<String> {
        catalog.agents.iter().map(|a| a.def.id.0.clone()).collect()
    }

    fn messages(catalog: &Catalog) -> String {
        catalog
            .problems
            .iter()
            .map(|p| format!("{}:{:?} {}\n", p.path.display(), p.line, p.message))
            .collect()
    }

    const DEVELOPER: &str = "---
name: developer
harness: claude
description: Implements one task end to end and reports back on it.
model: sonnet
effort: high
tools: Read, Edit, Write, Bash, Grep, Glob
permission-mode: acceptEdits
max-concurrent: 1
---

You are the developer agent for this project.

Work one task at a time.
";

    /// The whole point of the format: the prompt survives verbatim, with its paragraphs, and the
    /// switches around it are all readable.
    #[test]
    fn a_definition_round_trips_its_prompt_and_its_switches() {
        let dir = temp("roundtrip");
        write(&dir.join("project"), "developer.md", DEVELOPER);

        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert_eq!(ids(&catalog), ["developer"]);
        let loaded = agent(&catalog, "developer");

        assert_eq!(loaded.def.label, "Developer", "title-cased from the id");
        assert_eq!(loaded.def.harness, Harness::Claude);
        assert_eq!(
            loaded.def.description,
            "Implements one task end to end and reports back on it."
        );
        assert_eq!(loaded.def.model.as_deref(), Some("sonnet"));
        assert_eq!(loaded.effort.as_deref(), Some("high"));
        assert_eq!(
            loaded.tools,
            ["Read", "Edit", "Write", "Bash", "Grep", "Glob"]
        );
        assert_eq!(loaded.permission_mode.as_deref(), Some("acceptEdits"));
        assert_eq!(loaded.def.max_concurrent, 1);
        assert_eq!(loaded.def.unavailable, None);
        assert_eq!(loaded.origin, dir.join("project/developer.md"));
        assert_eq!(loaded.shadows, None);

        // The paragraph break is the reason this file is markdown and not JSON.
        assert_eq!(
            loaded.def.system_prompt,
            "You are the developer agent for this project.\n\nWork one task at a time."
        );
        assert!(catalog.errors().next().is_none(), "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The failure a hand-written parser exists to report well: the author forgot the second
    /// `---`, so their prompt is being read as front matter.
    ///
    /// It surfaces two ways, and both have to name the fence rather than the grammar. A file
    /// that is *only* front matter runs to EOF; a file with a prompt fails on the prompt's first
    /// prose line, which is not the line at fault.
    #[test]
    fn front_matter_that_is_never_closed_names_a_line() {
        let err = frontmatter::parse("---\nname: qa\ndescription: Checks things.\n")
            .expect_err("no closing fence");
        assert_eq!(err.line, 3);
        assert!(
            err.message.contains("never closed"),
            "unhelpful: {}",
            err.message
        );

        let text = "---\nname: qa\ndescription: Checks things.\n\nYou are the qa agent.\n";
        let err = frontmatter::parse(text).expect_err("no closing fence");
        assert_eq!(err.line, 5, "the prose line the parser choked on");
        assert!(
            err.message.contains("closing `---`"),
            "must name the fence, not the grammar: {}",
            err.message
        );

        // And through the loader, where it must be attributed to the file.
        let dir = temp("unclosed");
        write(&dir.join("project"), "qa.md", text);
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert!(catalog.agents.is_empty());
        let problem = &catalog.problems[0];
        assert_eq!(problem.path, dir.join("project/qa.md"));
        assert!(problem.line.is_some(), "a line number is the whole point");
        assert_eq!(problem.severity, Severity::Error);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cp developer.md reviewer.md` and forget the `name:` line. The role still loads, greyed,
    /// and the error names the file it should have been called.
    #[test]
    fn a_name_that_disagrees_with_the_file_name_is_reported_and_greyed() {
        let dir = temp("stem");
        write(
            &dir.join("project"),
            "reviewer.md",
            "---\nname: developer\ndescription: d\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );

        // Keyed by the *declared* name, which is what makes a stray copy detectable at all.
        assert_eq!(ids(&catalog), ["developer"]);
        let loaded = agent(&catalog, "developer");
        let reason = loaded.def.unavailable.as_deref().expect("greyed");
        assert!(reason.contains("reviewer.md"), "{reason}");
        assert!(
            messages(&catalog).contains("reviewer.md"),
            "{}",
            messages(&catalog)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The path-safety rule, at the layer that must never let a bad name reach a `Path::join`.
    #[test]
    fn a_name_that_could_escape_the_worktree_directory_is_refused_outright() {
        for bad in [
            "../evil",
            "..",
            "a/b",
            "-flag",
            "Developer",
            "dev_eloper",
            "",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", // 33
        ] {
            assert!(!valid_name(bad), "`{bad}` must be refused");
        }
        for good in ["developer", "qa", "a", "code-reviewer", "agent-2"] {
            assert!(valid_name(good), "`{good}` must be accepted");
        }

        // And the file it came from is dropped entirely rather than greyed: the name is the key
        // a worktree path and a branch ref are built from, so there is no safe identity to list.
        let dir = temp("evil");
        write(
            &dir.join("project"),
            "evil.md",
            "---\nname: ../../etc\ndescription: d\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert!(catalog.agents.is_empty(), "{:?}", ids(&catalog));
        assert!(
            messages(&catalog).contains(".cide/worktrees/"),
            "the refusal must say why it is a refusal: {}",
            messages(&catalog)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent with no prompt is the CLI's default agent wearing a label, which is the most
    /// confusing possible outcome — so it is an error, and the role is greyed rather than run.
    #[test]
    fn a_definition_with_no_system_prompt_is_an_error() {
        let dir = temp("empty-prompt");
        write(
            &dir.join("project"),
            "qa.md",
            "---\nname: qa\ndescription: Checks things.\n---\n\n   \n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );

        let loaded = agent(&catalog, "qa");
        assert!(loaded.def.system_prompt.is_empty());
        let reason = loaded.def.unavailable.as_deref().expect("greyed");
        assert!(reason.contains("no system prompt"), "{reason}");
        assert_eq!(catalog.errors().count(), 1, "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whole-file override, and the shadowed file is named. A silently shadowed global definition
    /// is the shape of a whole afternoon.
    #[test]
    fn a_project_definition_shadows_the_global_one_whole_file() {
        let dir = temp("shadow");
        write(
            &dir.join("global"),
            "developer.md",
            "---\nname: developer\ndescription: Global.\nmodel: opus\n\
             tools: Read\npermission-mode: manual\n---\nGlobal prompt.\n",
        );
        write(
            &dir.join("global"),
            "artist.md",
            "---\nname: artist\ndescription: Draws.\n---\nGlobal artist prompt.\n",
        );
        write(
            &dir.join("project"),
            "developer.md",
            "---\nname: developer\ndescription: Project.\n---\nProject prompt.\n",
        );

        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert_eq!(ids(&catalog), ["artist", "developer"], "sorted by id");

        let developer = agent(&catalog, "developer");
        assert_eq!(developer.def.system_prompt, "Project prompt.");
        assert_eq!(developer.def.description, "Project.");
        // The half of whole-file override that a field-by-field merge would get wrong: the
        // global's `model`, `tools` and `permission-mode` are gone, not inherited. A prompt
        // beside somebody else's safety switches is an agent nobody wrote.
        assert_eq!(developer.def.model, None);
        assert!(developer.tools.is_empty());
        assert_eq!(developer.permission_mode, None);

        assert_eq!(developer.origin, dir.join("project/developer.md"));
        assert_eq!(
            developer.shadows.as_deref(),
            Some(dir.join("global/developer.md").as_path()),
            "an unshadowed global edit is a whole afternoon"
        );
        // A global role the project said nothing about is still there, unshadowed.
        assert_eq!(agent(&catalog, "artist").shadows, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The discipline `Filter::build` uses for an unreadable `.gitignore`: one bad file is one
    /// finding, not an empty roster.
    #[test]
    fn one_broken_file_does_not_stop_the_other_two_loading() {
        let dir = temp("resilient");
        let project = dir.join("project");
        write(
            &project,
            "developer.md",
            "---\nname: developer\ndescription: d\n---\nOne.\n",
        );
        write(&project, "broken.md", "no front matter at all\n");
        write(
            &project,
            "qa.md",
            "---\nname: qa\ndescription: q\n---\nTwo.\n",
        );

        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        assert_eq!(ids(&catalog), ["developer", "qa"]);
        assert_eq!(catalog.errors().count(), 1, "{}", messages(&catalog));
        assert_eq!(catalog.problems[0].path, project.join("broken.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two files in one scope claiming one name. The error names both, and the file whose own
    /// name agrees with its `name:` key is the one that keeps working.
    #[test]
    fn two_files_declaring_one_name_is_an_error_naming_both() {
        let dir = temp("dupe");
        let project = dir.join("project");
        write(
            &project,
            "developer.md",
            "---\nname: developer\ndescription: original\n---\nOriginal.\n",
        );
        write(
            &project,
            "developer-old.md",
            "---\nname: developer\ndescription: copy\n---\nCopy.\n",
        );

        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        assert_eq!(ids(&catalog), ["developer"]);
        let text = messages(&catalog);
        assert!(text.contains("developer.md"), "{text}");
        assert!(text.contains("developer-old.md"), "{text}");

        // The stem-matching file wins, so `cp` + forget does not break the original.
        let loaded = agent(&catalog, "developer");
        assert_eq!(loaded.def.description, "original");
        assert_eq!(loaded.origin, project.join("developer.md"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// When *neither* file's name agrees with its key there is nothing to choose between them,
    /// so the survivor is greyed rather than guessed at.
    #[test]
    fn two_files_that_both_disagree_with_their_names_leave_the_role_greyed() {
        let dir = temp("dupe-ambiguous");
        let project = dir.join("project");
        write(
            &project,
            "a.md",
            "---\nname: developer\ndescription: a\n---\nA.\n",
        );
        write(
            &project,
            "b.md",
            "---\nname: developer\ndescription: b\n---\nB.\n",
        );

        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        let loaded = agent(&catalog, "developer");
        let reason = loaded.def.unavailable.as_deref().expect("greyed");
        assert!(
            reason.contains("a.md") && reason.contains("b.md"),
            "{reason}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "We could not tell" must stay distinguishable from "it is not there": the role is greyed
    /// with a sentence, never hidden.
    #[test]
    fn an_uninstalled_harness_greys_a_role_but_never_hides_it() {
        let dir = temp("harness");
        write(
            &dir.join("project"),
            "developer.md",
            "---\nname: developer\ndescription: d\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            missing,
        );
        let loaded = agent(&catalog, "developer");
        assert!(loaded.def.unavailable.is_some());
        assert!(!loaded.is_available());
        // Nothing was reported as a *problem with the file*: the file is fine, the machine is
        // not, and a red line against a definition the user cannot fix by editing would be a lie.
        assert!(catalog.problems.is_empty(), "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A harness this build does not have is the same greyed row with a different sentence, plus
    /// an error against the line — because that one *is* fixable by editing.
    #[test]
    fn an_unknown_harness_names_the_line_and_greys_the_role() {
        let dir = temp("unknown-harness");
        write(
            &dir.join("project"),
            "developer.md",
            "---\nname: developer\nharness: gemini\ndescription: d\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        let loaded = agent(&catalog, "developer");
        assert!(
            loaded
                .def
                .unavailable
                .as_deref()
                .is_some_and(|r| r.contains("gemini"))
        );
        assert_eq!(catalog.problems[0].line, Some(3));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A role on the second harness is dispatchable, and is still greyed when its binary is
    /// missing.** The two probes, on the harness that has one of each answer.
    ///
    /// This test replaces `a_harness_this_build_cannot_run_is_unavailable_even_with_its_binary_present`,
    /// which asserted the opposite of the first half and was marked to go when `OpencodeHarness`
    /// landed. It has landed, so the sentence that test pinned — *this build of cide cannot run
    /// “opencode” roles* — became false, and a test asserting a false sentence is worse than no
    /// test. What that one was really protecting is kept here in the half that is still real:
    /// **the machine probe**, which is the one a user can act on.
    ///
    /// [`implemented`] itself is not gone and is not vestigial; it is checked in
    /// [`the_build_and_the_machine_get_different_sentences`], which is where the invariant that
    /// outlived this test lives.
    #[test]
    fn a_role_on_the_second_harness_runs_but_still_needs_its_binary() {
        let dir = temp("second-harness");
        let project = dir.join("project");
        write(
            &project,
            "developer.md",
            "---\nname: developer\nharness: opencode\ndescription: d\n---\nPrompt.\n",
        );

        // The ordinary machine: `opencode` installed, the build has the harness, so the panel
        // draws a Dispatch button that works. Before the harness landed this same role loaded
        // greyed, with a sentence saying this build could not start it.
        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        let loaded = agent(&catalog, "developer");
        assert_eq!(loaded.def.harness, Harness::Opencode);
        assert_eq!(
            loaded.def.unavailable, None,
            "a sound opencode role on a machine that has the binary is dispatchable"
        );
        assert!(catalog.problems.is_empty(), "{}", messages(&catalog));

        // And the half that did not change: the binary probe still greys the role, with the
        // sentence that *is* an instruction. Listed, never hidden — "we could not tell" and "it
        // is not there" have to stay distinguishable, and a vanished role is neither.
        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, missing);
        let loaded = agent(&catalog, "developer");
        assert!(!loaded.def.system_prompt.is_empty(), "still fully loaded");
        let reason = loaded.def.unavailable.as_deref().expect("greyed");
        assert!(
            reason.contains("PATH"),
            "this one is the user's to fix by installing something: {reason}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Every harness the wire knows, this build can run** — and the sentence for one it could
    /// not is still a different sentence from the machine's.
    ///
    /// The first half is the invariant that replaced the old assertion here (`implemented`
    /// answering `Some` for `opencode`, which stopped being true when `OpencodeHarness` landed).
    /// It is not a tautology: [`implemented`] asks [`crate::harness::for_kind`], so this fails
    /// the moment a variant is added to [`Harness`] ahead of its implementation — which is the
    /// ordinary shape of a staged delivery and exactly what the function exists to catch. The
    /// role is then greyed with a sentence rather than dispatched into a `for_kind` that comes
    /// back empty three layers down, after a git worktree has already been cut.
    ///
    /// The second half is read straight off the two functions rather than through a catalog,
    /// because the claim is about the wording each one owns: a future edit that made them agree
    /// would put a user who cannot act in front of an instruction.
    #[test]
    fn the_build_and_the_machine_get_different_sentences() {
        for harness in [Harness::Claude, Harness::Opencode] {
            assert_eq!(
                implemented(harness),
                None,
                "{} is in the wire enum and has no implementation",
                harness_name(harness)
            );
        }

        // The sentence itself, which no variant produces today and which the next one will. Built
        // here rather than asserted through a role, so that the wording is checked even while
        // nothing reaches it — a sentence nobody can see is a sentence that rots.
        let build = format!(
            "this build of cide cannot run “{}” roles: it has no implementation",
            harness_name(Harness::Opencode)
        );
        assert!(unimplemented_sentence(Harness::Opencode).starts_with(&build));
        assert!(!unimplemented_sentence(Harness::Opencode).contains("PATH"));

        // `installed`'s own sentence is an instruction about the machine — the other half of the
        // pair, and the only half of it a user can act on.
        let machine = missing(Harness::Claude).expect("the test probe reports it absent");
        assert!(machine.contains("PATH"));
        assert_ne!(unimplemented_sentence(Harness::Opencode), machine);
    }

    /// The ordinary case, both ways round: the check added above must not grey `claude`.
    #[test]
    fn an_implemented_harness_is_greyed_only_when_its_binary_is_missing() {
        let dir = temp("claude-probe");
        let project = dir.join("project");
        write(
            &project,
            "developer.md",
            "---\nname: developer\nharness: claude\ndescription: d\n---\nPrompt.\n",
        );

        let present_catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        assert_eq!(
            agent(&present_catalog, "developer").def.unavailable,
            None,
            "a sound claude role on a machine that has the binary is dispatchable"
        );

        let missing_catalog = load_from(&dir.join("global"), &project, Harness::Claude, missing);
        let reason = agent(&missing_catalog, "developer")
            .def
            .unavailable
            .as_deref()
            .expect("greyed");
        assert!(
            reason.contains("PATH"),
            "this one *is* an instruction the user can follow: {reason}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two vocabularies that are not cide's, treated differently on purpose: a tool name is
    /// advisory, a permission mode is a safety switch.
    #[test]
    fn an_unknown_tool_warns_but_an_unknown_permission_mode_greys() {
        let dir = temp("vocab");
        write(
            &dir.join("project"),
            "developer.md",
            "---\nname: developer\ndescription: d\ntools: Read, Notebook, mcp__ide__openDiff\n\
             permission-mode: yolo\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        let loaded = agent(&catalog, "developer");

        let warnings: Vec<_> = catalog
            .problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
            .collect();
        assert_eq!(warnings.len(), 1, "{}", messages(&catalog));
        assert!(warnings[0].message.contains("Notebook"));
        // An MCP tool names something no list here could check it against.
        assert!(!messages(&catalog).contains("openDiff"));

        assert_eq!(catalog.errors().count(), 1);
        let reason = loaded.def.unavailable.as_deref().expect("greyed");
        assert!(
            reason.contains("yolo") && reason.contains("acceptEdits"),
            "{reason}"
        );
        assert_eq!(
            loaded.permission_mode, None,
            "a mode cide cannot vouch for is not carried"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two unknown keys, two answers: a near-miss of a real key is a typo that greys the role,
    /// anything else only warns so a definition from a newer cide still runs.
    #[test]
    fn a_misspelled_key_greys_the_role_and_an_unknown_one_only_warns() {
        let dir = temp("unknown-key");
        let project = dir.join("project");
        write(
            &project,
            "developer.md",
            "---\nname: developer\ndescription: d\npermission_mode: acceptEdits\n---\nP.\n",
        );
        write(
            &project,
            "qa.md",
            "---\nname: qa\ndescription: d\nfuture-thing: 3\n---\nP.\n",
        );
        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);

        let developer = agent(&catalog, "developer");
        let reason = developer.def.unavailable.as_deref().expect("greyed");
        assert!(reason.contains("permission-mode"), "{reason}");
        assert_eq!(
            developer.permission_mode, None,
            "the switch never took effect"
        );

        let qa = agent(&catalog, "qa");
        assert!(
            qa.is_available(),
            "a key from the future must not empty a roster"
        );
        let warnings: Vec<_> = catalog
            .problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
            .collect();
        assert_eq!(warnings.len(), 1, "{}", messages(&catalog));
        assert!(warnings[0].message.contains("future-thing"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Everything the restricted grammar refuses, each with its own sentence and its own line.
    #[test]
    fn the_grammar_refuses_what_it_cannot_read_rather_than_ignoring_it() {
        let cases = [
            ("---\nname: a\ntools:\n  - Read\n---\nP.\n", 4, "indented"),
            (
                "---\nname: a\n- Read\n---\nP.\n",
                3,
                "lists are not supported",
            ),
            ("---\nname: a\nbase: &anchor\n---\nP.\n", 3, "anchor"),
            ("---\nname: a\nmodel: *base\n---\nP.\n", 3, "alias"),
            ("---\nname: a\nmodel: !!str x\n---\nP.\n", 3, "tag"),
            ("---\nname: a\ndescription: |\n---\nP.\n", 3, "block scalar"),
            ("---\nname: a\nagents: {a: b}\n---\nP.\n", 3, "flow mapping"),
            ("---\nname: a\nnope\n---\nP.\n", 3, "expected `key: value`"),
            ("---\nname: a\nname: b\n---\nP.\n", 3, "set twice"),
            ("---\nname: a\nBad Key: x\n---\nP.\n", 3, "is not a key"),
            ("name: a\n", 1, "must begin with"),
        ];
        for (text, line, needle) in cases {
            let err = frontmatter::parse(text).expect_err(text);
            assert_eq!(err.line, line, "{text}");
            assert!(err.message.contains(needle), "{}: {}", text, err.message);
        }
    }

    /// The escape hatch: quoting means "literally this", so prose starting with a character the
    /// refusals above reject is still writable.
    #[test]
    fn a_quoted_value_is_taken_literally() {
        let doc = frontmatter::parse(
            "---\nname: a\ndescription: \"*emphasis* matters\"\nmodel: 'x'\n---\nP.\n",
        )
        .expect("parses");
        assert_eq!(doc.fields[1].value, "*emphasis* matters");
        assert_eq!(doc.fields[2].value, "x");
        // A `#` inside a value is part of the value: `fixes issue #42` is far likelier than a
        // trailing comment, and a whole-line comment exists for the other case.
        let doc = frontmatter::parse("---\n# a comment\nname: a\nlabel: b #42\n---\nP.\n")
            .expect("parses");
        assert_eq!(doc.fields.len(), 2, "the comment line is not a field");
        assert_eq!(doc.fields[1].value, "b #42");
    }

    /// Both separators, and the bracket form, because all three are what people write.
    #[test]
    fn a_list_may_be_comma_or_space_separated() {
        for text in ["Read, Edit,Write", "Read Edit Write", "[Read, Edit, Write]"] {
            assert_eq!(split_list(text), ["Read", "Edit", "Write"], "{text}");
        }
        assert!(split_list("").is_empty());
    }

    /// A roster is readable before anybody has thought about presentation.
    #[test]
    fn a_label_is_title_cased_from_the_id_unless_the_file_gives_one() {
        assert_eq!(label_from_id("developer"), "Developer");
        assert_eq!(label_from_id("code-reviewer"), "Code Reviewer");
        assert_eq!(label_from_id("qa"), "Qa");

        let dir = temp("label");
        write(
            &dir.join("project"),
            "qa.md",
            "---\nname: qa\nlabel: QA\ndescription: d\n---\nPrompt.\n",
        );
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert_eq!(agent(&catalog, "qa").def.label, "QA");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A project that has never used this feature has no `.cide/agents/`, and that is not a
    /// finding — it is every repository in the world.
    #[test]
    fn a_missing_directory_is_silent() {
        let dir = temp("absent");
        let catalog = load_from(
            &dir.join("global"),
            &dir.join("project"),
            Harness::Claude,
            present,
        );
        assert!(catalog.agents.is_empty());
        assert!(catalog.problems.is_empty(), "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The config's harness is the default; the definition's own key still wins.
    #[test]
    fn a_definition_without_a_harness_takes_the_projects_default() {
        let dir = temp("default-harness");
        let project = dir.join("project");
        write(&project, "a.md", "---\nname: a\ndescription: d\n---\nP.\n");
        write(
            &project,
            "b.md",
            "---\nname: b\nharness: claude\ndescription: d\n---\nP.\n",
        );
        let catalog = load_from(&dir.join("global"), &project, Harness::Opencode, present);
        assert_eq!(agent(&catalog, "a").def.harness, Harness::Opencode);
        assert_eq!(agent(&catalog, "b").def.harness, Harness::Claude);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `max-concurrent: 0` would define a role that can never run, which nobody means.
    #[test]
    fn a_max_concurrent_that_cannot_run_is_refused_and_falls_back_to_one() {
        let dir = temp("concurrency");
        let project = dir.join("project");
        write(
            &project,
            "a.md",
            "---\nname: a\ndescription: d\nmax-concurrent: 0\n---\nP.\n",
        );
        write(
            &project,
            "b.md",
            "---\nname: b\ndescription: d\nmax-concurrent: lots\n---\nP.\n",
        );
        let catalog = load_from(&dir.join("global"), &project, Harness::Claude, present);
        assert_eq!(agent(&catalog, "a").def.max_concurrent, 1);
        assert_eq!(agent(&catalog, "b").def.max_concurrent, 1);
        assert_eq!(catalog.errors().count(), 2, "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The harness spellings are the wire's, in both directions, so a file and a DTO agree.
    #[test]
    fn harness_names_round_trip() {
        for harness in [Harness::Claude, Harness::Opencode] {
            assert_eq!(harness_from_str(harness_name(harness)), Some(harness));
            assert_eq!(
                serde_json::to_string(&harness).unwrap(),
                format!("\"{}\"", harness_name(harness))
            );
        }
        assert_eq!(harness_from_str("Claude"), None);
    }

    // ======================================================================================
    // The writer.
    // ======================================================================================

    /// A draft with the two fields every valid one must have and nothing else.
    fn draft(name: &str, prompt: &str) -> AgentDraft {
        AgentDraft {
            scope: AgentScope::Project,
            name: AgentId(name.to_string()),
            original: None,
            label: None,
            harness: None,
            description: String::new(),
            model: None,
            effort: None,
            tools: Vec::new(),
            permission_mode: None,
            max_concurrent: None,
            system_prompt: prompt.to_string(),
        }
    }

    /// The table the round-trip is asserted over, each row a way the grammar can be provoked.
    ///
    /// A table rather than one example, because every one of these was reasoned about
    /// separately and a single "typical" draft would exercise none of them. `.0` is what the row
    /// is for, so a failure names the property that broke rather than an index.
    ///
    /// Every row is a draft [`validate`] accepts, which is the population the invariant is
    /// stated over: an invalid draft has no file to round-trip through, and the refusals are
    /// asserted separately in `a_refusal_names_the_field_it_belongs_to`.
    fn awkward_drafts() -> Vec<(&'static str, AgentDraft)> {
        // Seeded with one row and grown by `push`, rather than written as one `vec![]`, because
        // the loop further down extends the same list and each row wants the sentence saying
        // which rule it is provoking sitting next to it.
        let mut rows: Vec<(&'static str, AgentDraft)> = vec![(
            "the minimum a definition can be",
            draft("qa", "Check things."),
        )];

        rows.push((
            "every key at once",
            AgentDraft {
                label: Some("Developer".into()),
                harness: Some(Harness::Claude),
                description: "Implements one task end to end.".into(),
                model: Some("sonnet".into()),
                effort: Some("high".into()),
                tools: vec![
                    "Read".into(),
                    "Edit".into(),
                    "mcp__cide__cide_task_list".into(),
                ],
                permission_mode: Some("acceptEdits".into()),
                max_concurrent: Some(u16::MAX),
                ..draft("developer", "You are the developer.")
            },
        ));

        // --- the body, which is the half the fence is a risk to ---------------------------
        rows.push((
            "a body whose first line is the fence",
            draft("a", "---\nnot front matter, prose."),
        ));
        rows.push((
            "a fence in the middle of the body",
            draft("b", "One.\n\n---\n\nTwo."),
        ));
        rows.push((
            "a body line that looks like a key",
            draft("c", "model: sonnet\n\nThat line is prose."),
        ));
        rows.push((
            "a body that is nothing but grammar",
            draft("d", "---\nname: not-me\n---\n- item\n  indented\n# comment"),
        ));
        rows.push((
            "quotes and backslashes in the body",
            draft("e", "He said \"hi\" and wrote C:\\\\Users\\\\me — '---'."),
        ));
        rows.push((
            "CRLF and a trailing newline",
            draft("f", "line one\r\nline two\r\n\r\n"),
        ));
        rows.push(("a lone carriage return", draft("g", "before\rafter")));
        rows.push((
            "leading and trailing blank space around the body",
            draft("h", "\n\n   Prompt.   \n\n"),
        ));
        rows.push((
            "a body that keeps its interior indentation",
            draft("i", "Fenced code:\n\n```\n  indented\n```"),
        ));

        // --- scalars, which is the half `unquote` is a risk to -----------------------------
        for (index, value) in [
            "*emphasis* leads the line",
            "&anchor",
            "!tag",
            "|block",
            ">folded",
            "{flow: mapping}",
            "\"quoted\"",
            "'single'",
            "\"",
            "\"\"",
            "''",
            "\"unbalanced",
            "unbalanced\"",
            "a colon: and more",
            "# not a comment",
            "--- not a fence",
            "-",
            "trailing hash #42",
            "tab\there",
        ]
        .into_iter()
        .enumerate()
        {
            rows.push((
                "a description the scalar grammar could misread",
                AgentDraft {
                    description: value.to_string(),
                    // The same string in a second scalar, so the quoting rule is not asserted
                    // against one field's spelling of it.
                    label: Some(value.to_string()),
                    ..draft(&format!("s{index}"), "Prompt.")
                },
            ));
        }

        // --- names at the edges of what `valid_name` allows --------------------------------
        rows.push(("a one-character name", draft("z", "Prompt.")));
        rows.push((
            "a name at the 32-character ceiling",
            draft("a2345678901234567890123456789012", "Prompt."),
        ));
        rows.push(("a name of digits and dashes", draft("7-of-9", "Prompt.")));

        // --- the optionals, which must stay distinguishable from their defaults ------------
        rows.push((
            "a harness named explicitly",
            AgentDraft {
                harness: Some(Harness::Opencode),
                ..draft("k", "Prompt.")
            },
        ));
        rows.push((
            "max-concurrent set to the value it defaults to",
            AgentDraft {
                max_concurrent: Some(1),
                ..draft("l", "Prompt.")
            },
        ));
        rows.push((
            "every permission mode cide knows",
            AgentDraft {
                permission_mode: Some(PERMISSION_MODES[0].to_string()),
                ..draft("m", "Prompt.")
            },
        ));
        rows.push((
            "one tool, so the list separator is never exercised",
            AgentDraft {
                tools: vec!["Read".into()],
                ..draft("n", "Prompt.")
            },
        ));
        rows.push((
            "the global scope",
            AgentDraft {
                scope: AgentScope::Global,
                ..draft("o", "Prompt.")
            },
        ));

        rows
    }

    /// **The invariant the writer exists to hold**: whatever a form put in, the file gives back.
    ///
    /// Stated against [`normalize`] rather than against the draft itself because the grammar has
    /// a fixed point — a scalar is trimmed by the parser, a body is trimmed and CRLF-folded —
    /// so `normalize` is asserted to *be* one (idempotent) before it is used as the right-hand
    /// side. Without that, this test could pass by both sides agreeing on nonsense.
    #[test]
    fn every_awkward_draft_survives_a_trip_through_its_own_file() {
        for (what, original) in awkward_drafts() {
            let normal = normalize(&original);
            assert_eq!(
                normalize(&normal),
                normal,
                "normalize is not idempotent, so it is not a fixed point: {what}"
            );
            assert_eq!(
                render(&normal),
                render(&original),
                "render does not normalize what it is given: {what}"
            );

            let text = render(&original);
            let back = parse_draft(&text, original.scope)
                .unwrap_or_else(|error| panic!("{what}: {}: {}", error.line, error.message));
            assert_eq!(back, normal, "{what}\n--- rendered ---\n{text}");

            // And once more from the file the round trip produced, which is the state every
            // definition is in from the second save onwards.
            assert_eq!(
                parse_draft(&render(&back), back.scope).expect("second render parses"),
                normal,
                "the second trip differs from the first: {what}"
            );
        }
    }

    /// The reader's *schema* layer accepts what the writer emits, not only its grammar.
    ///
    /// A separate assertion from the round trip because they can fail apart: a file can parse as
    /// front matter and still be greyed by [`read_definition`] for a name that disagrees with its
    /// stem, an unrecognised permission mode, or an empty prompt. A writer whose output the
    /// loader refuses would produce a role the user created and cide will not run.
    #[test]
    fn what_the_writer_emits_is_a_role_the_loader_lists() {
        let dir = temp("rendered");
        for (index, (what, original)) in awkward_drafts().into_iter().enumerate() {
            let normal = normalize(&original);
            let project = dir.join(index.to_string()).join("project");
            write(&project, &format!("{}.md", normal.name), &render(&original));

            let catalog = load_from(
                &dir.join(index.to_string()).join("global"),
                &project,
                Harness::Opencode,
                present,
            );
            assert!(
                catalog.errors().next().is_none(),
                "{what}: {}",
                messages(&catalog)
            );
            let loaded = agent(&catalog, normal.name.as_str());
            assert_eq!(loaded.def.system_prompt, normal.system_prompt, "{what}");
            assert_eq!(loaded.def.description, normal.description, "{what}");
            assert_eq!(loaded.tools, normal.tools, "{what}");
            assert_eq!(loaded.permission_mode, normal.permission_mode, "{what}");
            assert_eq!(loaded.effort, normal.effort, "{what}");
            assert_eq!(loaded.def.model, normal.model, "{what}");
            // The two fields where "the file is silent" and "the file says this" differ, which is
            // the reason `parse_draft` exists beside `read_definition`.
            assert_eq!(
                loaded.def.harness,
                normal.harness.unwrap_or(Harness::Opencode),
                "{what}"
            );
            assert_eq!(
                loaded.def.max_concurrent,
                normal.max_concurrent.unwrap_or(1),
                "{what}"
            );
            assert_eq!(
                loaded.def.label,
                normal
                    .label
                    .clone()
                    .unwrap_or_else(|| label_from_id(normal.name.as_str())),
                "{what}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Front matter carries only keys the parser reads, in the order it lists them.
    ///
    /// The order is cosmetic and the *membership* is not: a key outside [`KNOWN_KEYS`] is a line
    /// the loader would warn about on a file cide itself wrote, which is the one place a warning
    /// is indefensible.
    #[test]
    fn the_front_matter_holds_known_keys_only_and_in_their_listed_order() {
        for (what, original) in awkward_drafts() {
            let text = render(&original);
            let doc = frontmatter::parse(&text).expect("renders parseable front matter");
            let mut expected = KNOWN_KEYS.iter();
            for field in &doc.fields {
                assert!(
                    expected.any(|known| *known == field.key),
                    "`{}` is not a known key, or is out of KNOWN_KEYS order: {what}",
                    field.key
                );
            }
            assert_eq!(doc.fields.first().map(|f| f.key.as_str()), Some("name"));
        }
    }

    /// The two halves of a definition file are the two things a `---` can mean, and the writer
    /// keeps them apart by putting the body where nothing reads grammar.
    #[test]
    fn a_body_cannot_close_its_own_front_matter() {
        let text = render(&AgentDraft {
            description: "one".into(),
            ..draft(
                "x",
                "---\nname: someone-else\ndescription: two\n---\nStill the body.",
            )
        });
        let doc = frontmatter::parse(&text).expect("parses");
        // The body's `name:` and `description:` lines are prose: the scanner stopped at the fence
        // `render` emitted, and everything after it is data.
        assert_eq!(doc.fields.len(), 2, "{text}");
        assert_eq!(doc.fields[0].value, "x");
        assert_eq!(doc.fields[1].value, "one");
        assert!(
            doc.body.starts_with("---\nname: someone-else"),
            "{}",
            doc.body
        );
    }

    /// A line break in a one-line value is the one thing that could put a body inside the front
    /// matter, and `render` folds it rather than trusting a caller to have validated.
    #[test]
    fn a_newline_in_a_value_cannot_open_a_line() {
        let text = render(&AgentDraft {
            description: "first\n---\nname: injected\ndescription: second".into(),
            ..draft("x", "Prompt.")
        });
        let doc = frontmatter::parse(&text).expect("parses");
        assert_eq!(doc.fields.len(), 2, "{text}");
        assert_eq!(
            doc.fields[1].value,
            "first --- name: injected description: second"
        );
        assert_eq!(doc.body, "Prompt.");
    }

    /// Comments and keys cide does not read are dropped by a save, which is a decision rather
    /// than an oversight — see [`parse_draft`]'s doc — and is pinned so it cannot become one.
    #[test]
    fn a_save_rewrites_the_file_to_what_cide_reads_and_no_more() {
        let text = "---\n# who wrote this\nname: qa\ndescription: Checks.\nfuture-key: value\n\
                    harness: bogus\nmax-concurrent: lots\n---\nPrompt.\n";
        let back = parse_draft(text, AgentScope::Project).expect("parses");
        assert_eq!(back.description, "Checks.");
        assert_eq!(back.harness, None, "an unreadable value reads as absent");
        assert_eq!(back.max_concurrent, None);

        let rendered = render(&back);
        assert!(!rendered.contains("future-key"), "{rendered}");
        assert!(!rendered.contains("who wrote this"), "{rendered}");
        assert!(!rendered.contains("bogus"), "{rendered}");
    }

    /// Every refusal reaches the box that carries it. The whole reason a rejection is a list of
    /// fields rather than a sentence.
    #[test]
    fn a_refusal_names_the_field_it_belongs_to() {
        let cases: Vec<(AgentField, AgentDraft)> = vec![
            (AgentField::Name, draft("", "Prompt.")),
            (AgentField::Name, draft("../escape", "Prompt.")),
            (AgentField::Name, draft("UPPER", "Prompt.")),
            (AgentField::Name, draft("-leading", "Prompt.")),
            (AgentField::SystemPrompt, draft("qa", "   \n\n  ")),
            (
                AgentField::PermissionMode,
                AgentDraft {
                    permission_mode: Some("acceptedits".into()),
                    ..draft("qa", "Prompt.")
                },
            ),
            (
                AgentField::MaxConcurrent,
                AgentDraft {
                    max_concurrent: Some(0),
                    ..draft("qa", "Prompt.")
                },
            ),
            (
                AgentField::Tools,
                AgentDraft {
                    tools: vec!["Read File".into()],
                    ..draft("qa", "Prompt.")
                },
            ),
            (
                AgentField::Tools,
                AgentDraft {
                    tools: vec!["[Read]".into()],
                    ..draft("qa", "Prompt.")
                },
            ),
        ];

        for (field, bad) in cases {
            let problems = validate(&bad);
            assert_eq!(
                problems.iter().map(|p| p.field).collect::<Vec<_>>(),
                vec![field],
                "expected one problem on {field:?} for {bad:?}"
            );
            // And the sentence says something the person at that box can act on.
            assert!(problems[0].message.len() > 40, "{:?}", problems[0]);
        }

        // Several at once, because a form that reports one at a time is a form pressed four
        // times to learn four things.
        let problems = validate(&AgentDraft {
            permission_mode: Some("nope".into()),
            max_concurrent: Some(0),
            ..draft("Bad Name", "")
        });
        assert_eq!(problems.len(), 4, "{problems:?}");

        // A warning is not a refusal: an unknown tool name and a missing description are the
        // roster's business, and a form that blocked on them would block every MCP tool.
        assert!(
            validate(&AgentDraft {
                tools: vec!["Notebook".into(), "mcp__x__y".into()],
                ..draft("qa", "Prompt.")
            })
            .is_empty()
        );
    }

    /// A save writes the file the draft names, at a mode a teammate can read.
    #[test]
    fn a_save_publishes_the_definition_where_the_loader_will_find_it() {
        let dir = temp("save");
        let root = dir.join("repo");

        let path = save(&root, &draft("qa", "Check things.")).expect("saved");
        assert_eq!(path, root.join(".cide/agents/qa.md"));

        let catalog = load_from(
            &dir.join("global"),
            &project_dir(&root),
            Harness::Claude,
            present,
        );
        assert_eq!(ids(&catalog), ["qa"]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
            // A ceiling, never a floor: `write_atomic_with_mode` asks for 0644 and the umask may
            // only narrow it. Asserting equality would fail under `umask 077`, which is a
            // configuration cide has no business overriding — see that function's own doc.
            assert_eq!(
                mode & !0o644,
                0,
                "wider than a shared file may be: {mode:o}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A name change is a **move**, and the order it happens in is what makes a crash survivable.
    #[test]
    fn renaming_a_role_moves_its_file_and_never_leaves_nothing_behind() {
        let dir = temp("rename");
        let root = dir.join("repo");
        save(&root, &draft("qa", "Check things.")).expect("saved");

        let renamed = AgentDraft {
            original: Some(AgentLocation {
                scope: AgentScope::Project,
                name: AgentId("qa".into()),
            }),
            ..draft("reviewer", "Check things.")
        };
        let path = save(&root, &renamed).expect("renamed");
        assert_eq!(path, root.join(".cide/agents/reviewer.md"));
        assert!(
            !root.join(".cide/agents/qa.md").exists(),
            "the old file is gone"
        );

        // The `name:` key moved with the file, which is the invariant `read_definition` enforces
        // and the reason a rename cannot be an in-place write.
        let catalog = load_from(
            &dir.join("global"),
            &project_dir(&root),
            Harness::Claude,
            present,
        );
        assert_eq!(ids(&catalog), ["reviewer"]);
        assert!(catalog.errors().next().is_none(), "{}", messages(&catalog));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scope change is the same move, between the two directories `load_from` merges.
    ///
    /// Asserted on the paths rather than performed, because [`global_dir`] is a real XDG lookup
    /// and a test that wrote into the developer's own `~/.config/cide/agents/` would be a test
    /// that edits their roles. The move *mechanism* is one code path and
    /// `renaming_a_role_moves_its_file_and_never_leaves_nothing_behind` exercises it; what is
    /// specific to a scope change is only that the two paths differ, which is what makes `save`
    /// treat it as a move rather than an in-place write.
    #[test]
    fn changing_scope_lands_on_a_different_file_and_is_therefore_a_move() {
        let root = std::env::temp_dir().join("cide-agents-scope");
        assert_eq!(
            scope_dir(&root, AgentScope::Global),
            global_dir(),
            "the global scope ignores the project root, by design"
        );
        assert_eq!(
            scope_dir(&root, AgentScope::Project),
            root.join(".cide/agents")
        );
        assert_ne!(
            definition_path(&root, AgentScope::Global, "qa"),
            definition_path(&root, AgentScope::Project, "qa"),
        );
        // And the guard is on the join in both scopes, not only the project one.
        assert_eq!(
            definition_path(&root, AgentScope::Global, "../escape"),
            None
        );
        assert_eq!(definition_path(&root, AgentScope::Project, "Q"), None);
    }

    /// A name that is taken is refused, and the refusal points at the box the user changed.
    #[test]
    fn a_save_will_not_write_over_a_definition_it_did_not_open() {
        let dir = temp("taken");
        let root = dir.join("repo");
        save(&root, &draft("qa", "Original prompt.")).expect("saved");

        // A create that would land on it.
        let problems = match save(&root, &draft("qa", "Replacement.")) {
            Err(WriteError::Rejected(problems)) => problems,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, AgentField::Name);

        // A rename onto it.
        save(&root, &draft("reviewer", "Another prompt.")).expect("saved");
        let collide = AgentDraft {
            original: Some(AgentLocation {
                scope: AgentScope::Project,
                name: AgentId("reviewer".into()),
            }),
            ..draft("qa", "Another prompt.")
        };
        assert!(matches!(
            save(&root, &collide),
            Err(WriteError::Rejected(_))
        ));

        // And nothing moved: both files are still there, with their own prompts.
        let catalog = load_from(
            &dir.join("global"),
            &project_dir(&root),
            Harness::Claude,
            present,
        );
        assert_eq!(ids(&catalog), ["qa", "reviewer"]);
        assert_eq!(agent(&catalog, "qa").def.system_prompt, "Original prompt.");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Populating the form: what the file says, plus which file said it.
    #[test]
    fn a_draft_read_back_remembers_the_file_it_came_from() {
        let dir = temp("read-draft");
        let root = dir.join("repo");
        save(
            &root,
            &AgentDraft {
                description: "Checks things.".into(),
                tools: vec!["Read".into(), "Grep".into()],
                ..draft("qa", "Check things.")
            },
        )
        .expect("saved");

        let back = read_draft(&root, AgentScope::Project, "qa").expect("read");
        assert_eq!(
            back.original,
            Some(AgentLocation {
                scope: AgentScope::Project,
                name: AgentId("qa".into())
            })
        );
        assert_eq!(back.tools, ["Read", "Grep"]);
        // Absent keys stay absent, so a save does not freeze today's defaults into the file.
        assert_eq!(back.harness, None);
        assert_eq!(back.max_concurrent, None);
        assert_eq!(back.label, None);

        // Saving it unchanged is a no-op on disk, which is the only way a form can be trusted.
        let before = std::fs::read_to_string(root.join(".cide/agents/qa.md")).expect("read");
        save(&root, &back).expect("re-saved");
        assert_eq!(
            std::fs::read_to_string(root.join(".cide/agents/qa.md")).expect("read"),
            before
        );

        // A file whose `name:` disagrees with its stem comes back as a *rename waiting to be
        // saved*, which is the repair `read_definition`'s error asks the user to make.
        write(
            &project_dir(&root),
            "copy.md",
            "---\nname: qa\ndescription: d\n---\nPrompt.\n",
        );
        let mismatched = read_draft(&root, AgentScope::Project, "copy").expect("read");
        assert_eq!(mismatched.name, AgentId("qa".into()));
        assert_eq!(
            mismatched.original.as_ref().map(|o| o.name.as_str()),
            Some("copy")
        );

        assert!(matches!(
            read_draft(&root, AgentScope::Project, "nobody"),
            Err(WriteError::NotFound(_))
        ));
        assert!(matches!(
            read_draft(&root, AgentScope::Project, "../escape"),
            Err(WriteError::Rejected(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A delete names one file, and says so when there was not one.
    #[test]
    fn a_delete_is_scoped_and_reports_a_file_that_was_not_there() {
        let dir = temp("delete");
        let root = dir.join("repo");
        save(&root, &draft("qa", "Check things.")).expect("saved");

        assert_eq!(
            delete(&root, "qa", AgentScope::Project).expect("deleted"),
            root.join(".cide/agents/qa.md")
        );
        assert!(!root.join(".cide/agents/qa.md").exists());

        assert!(matches!(
            delete(&root, "qa", AgentScope::Project),
            Err(WriteError::NotFound(_))
        ));
        // The join is guarded here too: an `unlink` on an unchecked name is worse than a write.
        assert!(matches!(
            delete(&root, "../../etc/passwd", AgentScope::Project),
            Err(WriteError::Rejected(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
