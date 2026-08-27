//! OpenSpec's wire types: what cide shows of an `openspec/` directory. (M28)
//!
//! [OpenSpec](https://github.com/Fission-AI/OpenSpec) is a tool-agnostic convention for
//! spec-driven development. `openspec/specs/<capability>/spec.md` holds the requirements as they
//! stand; `openspec/changes/<name>/` holds one piece of proposed work — a proposal, a design, a
//! `tasks.md` checklist, and *delta* spec files stating `## ADDED | MODIFIED | REMOVED |
//! RENAMED Requirements`. Archiving a change merges its deltas into `specs/`.
//!
//! # These types are cide's contract, not OpenSpec's
//!
//! Everything here is what cide's frontend is promised. It is **not** the JSON the `openspec`
//! CLI emits: those shapes live in `cide_spec::model`, are `Deserialize`-only, and are versioned
//! by npm rather than by this repository. The two are deliberately separate for the reason
//! `cide-tasks` keeps *no* second copy of [`crate::Task`] — but arrived at from the opposite
//! direction. `.cide/tasks.json` is cide's own file, so one type can be domain, disk and wire at
//! once; `openspec/` is somebody else's format read through somebody else's tool, so a change in
//! their JSON must be able to land in one crate without touching the wire the panels are built
//! against.
//!
//! The translation is therefore allowed to be lossy in one direction and must be *total* in the
//! other: `cide-spec` flattens, renames and drops, and everything that survives is a field a
//! panel draws.
//!
//! # What cide deliberately does not model
//!
//! * **Workflow schemas.** `openspec/config.yaml` selects one, and its artifacts are declared
//!   with `generates` globs — so even "tasks.md" is not a filename cide may assume. [`SpecArtifact`]
//!   carries the *resolved* paths the CLI reports and nothing about how they were resolved.
//! * **Stores.** OpenSpec can resolve its root to a registered standalone repository. cide never
//!   passes `--store`: a project's specs are the ones in the project, and a board that silently
//!   described a different repository is the failure mode that would be hardest to notice.
//! * **A scenario's structure.** A scenario is one string, because that is all the CLI itself
//!   models it as (`{ rawText }`). Parsing WHEN/THEN into fields would need a serialiser that
//!   round-trips every scenario anybody ever hand-wrote, and the first that did not would
//!   silently rewrite a committed file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::{ChangeName, ProjectId, SpecId};

/// What cide can say about a project's `openspec/` right now.
///
/// Three shapes rather than an empty list, on [`crate::TaskBoard`]'s argument: an empty board
/// cannot distinguish "this project does not use OpenSpec", "the tool that reads it is not
/// installed" and "it is set up and there is nothing in flight", and those are three different
/// screens with three different next actions.
///
/// # Why `Absent` is decided by cide and not by the CLI
///
/// It has to be. `openspec list --json` in a directory with no `openspec/` exits **0** with an
/// empty list and `root.source: "implicit"` — indistinguishable, from the exit code alone, from
/// a project that is set up and has proposed nothing. Worse, the CLI's root resolution *walks
/// ancestors*, so a directory nested inside a repository that does have one will happily report
/// the parent's board. `cide-spec` therefore stats the directory itself and pins the resolved
/// root against the directory it asked about; see its `NoRoot` error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecBoard {
    /// No `openspec/` in this project. `hint` is the sentence the panel prints above its
    /// Set-up button, and `path` is the directory that would be created — named, because a
    /// control that adds a tracked folder to somebody's repository must say where.
    Absent {
        hint: String,
        #[ts(type = "string")]
        path: PathBuf,
    },
    /// There is an `openspec/`, and cide could not read it: the CLI is not installed, or it
    /// refused, or it resolved a root that is not this project's.
    ///
    /// One arm for all three because the panel does the same thing with each — prints the
    /// sentence and offers Retry — and because the sentences are built where the failure is
    /// known. Splitting it into a taxonomy would put a `match` in the frontend over cases only
    /// `cide-spec` can tell apart.
    Unusable { reason: String },
    Ready {
        changes: Vec<ChangeSummary>,
        specs: Vec<SpecSummary>,
        /// The directory the CLI resolved, echoed so the panel can prove it is this project's.
        #[ts(type = "string")]
        root: PathBuf,
        /// Which of OpenSpec's own workflow commands this project has, and the line that runs
        /// each — see [`SpecCommand`]. Empty is a real state and not an error: a project can
        /// have an `openspec/` and no Claude Code surface at all.
        commands: Vec<SpecCommand>,
    },
}

/// One of OpenSpec's workflow commands, as this project can actually invoke it.
///
/// # Why the invocation is carried and not derived
///
/// Because it has changed under cide once already, silently, and it will change again. OpenSpec
/// installed its workflow as **slash commands** in `.claude/commands/opsx/`, so `propose` was
/// `/opsx:propose` and cide spelled that prefix out in nine places. Somewhere before 1.7 the CLI
/// moved the same workflow to **Claude Code skills** — `.claude/skills/openspec-propose/SKILL.md`,
/// invoked as `/openspec-propose` — and every one of those nine places became a name no project
/// has. The panel's buttons then refused with a sentence telling the user to run `openspec
/// update`, which on a current CLI answers *"all tools up to date"* and writes nothing: a
/// dead end, in cide's own words, for a project that was set up correctly.
///
/// So `name` is cide's own handle for the command (`propose`) — stable, used by
/// `spec_run_command` and by the panel's buttons — and `line` is what a *this* project types,
/// read from its directory. A third surface costs one arm in `cide_spec::claude` and nothing
/// anywhere else.
///
/// # Why the panel is given the line rather than trusted with it
///
/// It draws the line as a preview under the composer, and a preview that differed from what is
/// sent would be its own small lie. `spec_run_command` still resolves `name` against the
/// directory itself and never types a string the frontend handed it — the wire carries this so
/// the two agree, not so one of them can be told what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecCommand {
    /// cide's handle: `propose`, `explore`. Kebab, and never carries a surface's prefix.
    pub name: String,
    /// What to type, prefix and all: `/openspec-propose`, or `/opsx:propose` on a project set up
    /// by an older CLI.
    pub line: String,
}

/// One active change, as the board lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangeSummary {
    pub name: ChangeName,
    /// Ticked and total checkboxes across the change's tracked-tasks artifact.
    ///
    /// **`total == 0` does not mean "complete"**, it means the checklist has no tasks yet, and
    /// the two must never be rendered the same. The Review hop keys off exactly this pair and
    /// fires only on `total > 0 && completed == total`.
    pub completed_tasks: u32,
    pub total_tasks: u32,
    /// The CLI's own word for where the change is (`no-tasks`, `in-progress`, `complete`).
    /// Carried as a string rather than an enum: it is upstream's vocabulary, it can grow in a
    /// point release, and a variant cide has not heard of must render as itself rather than
    /// vanish into an `Unknown` arm.
    pub status: String,
    #[ts(optional)]
    pub last_modified: Option<String>,
}

/// One capability under `openspec/specs/`, as the board lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecSummary {
    pub id: SpecId,
    pub requirement_count: u32,
}

/// Everything the card shows about one change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecChange {
    pub name: ChangeName,
    /// What the CLI calls it, which for a change with no explicit title is its own name.
    pub title: String,
    /// The prose is **not** here, and that is a fact about the CLI rather than a choice.
    ///
    /// `openspec show <change> --json` carries the change's *deltas* and nothing of its
    /// proposal or design — those are files, and cide reads them on demand through
    /// `spec_artifact`. Which is the right shape anyway: the panel draws those sections
    /// collapsed, so fetching a document nobody has expanded would be a file read per board
    /// refresh for prose nobody is looking at.
    pub deltas: Vec<SpecDelta>,
    /// The change's artifacts, with the paths the CLI resolved for them.
    pub artifacts: Vec<SpecArtifact>,
    pub progress: SpecProgress,
    /// The strict-validation verdict at the moment this was read.
    ///
    /// Meaningless for an archived change and set to *valid with no issues* there — see
    /// [`SpecOrigin::Archived`], which every surface checks first.
    pub validation: SpecValidation,
    /// Which of the two directories this was read from, and by which route.
    pub origin: SpecOrigin,
}

/// Where a [`SpecChange`] came from. (M28)
///
/// # Why an archived change is a different kind of answer, not a missing one
///
/// `openspec archive` **moves** a change to `openspec/changes/archive/<YYYY-MM-DD>-<name>/` and
/// merges its deltas into the specs. The directory survives intact — proposal, design, checklist
/// and delta files, all of it — but no CLI command reads it: `show` resolves
/// `openspec/changes/<name>/proposal.md` and nothing else, so an archived change answers
/// *not found*. A task linked to one therefore rendered as a failure ("could not be read, it may
/// have been archived") from the moment the work was accepted, which is precisely the moment its
/// record matters most.
///
/// So cide reads the directory itself, and this field is the honest label on what comes back: the
/// deltas are recovered with the same byte-range scanner the write path uses, and the two fields
/// no directory listing can answer — [`SpecChange::progress`] and [`SpecChange::validation`] —
/// carry neutral values that **no surface may draw** for this arm. A progress bar over `0/0` on a
/// change that is finished, or a green *valid* nobody validated, would each be a worse lie than
/// the refusal this replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecOrigin {
    /// `openspec/changes/<name>/`, read through the CLI. Everything is answered.
    Active,
    /// `openspec/changes/archive/<folder>/`, read off the disk by cide.
    Archived {
        /// The directory's own name, date stamp and all — the only place it is recoverable, and
        /// what a user needs to find the files on disk.
        folder: String,
    },
}

/// What one delta file does to one capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecDelta {
    pub spec: SpecId,
    pub operation: DeltaOperation,
    pub description: String,
    /// The requirements this delta carries.
    ///
    /// **One vector, where the CLI has two spellings.** Its JSON carries `requirement` for a
    /// single one and `requirements` for a list, and both mean the same thing; `cide-spec`
    /// folds them here. Two spellings of one fact reaching the frontend would be two code paths
    /// in a renderer, and the one that is exercised less would rot.
    pub requirements: Vec<SpecRequirement>,
    /// Set only for [`DeltaOperation::Renamed`].
    #[ts(optional)]
    pub rename: Option<SpecRename>,
}

/// What a delta does. OpenSpec's four operations, and the set is closed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DeltaOperation {
    Added,
    Modified,
    Removed,
    Renamed,
}

impl DeltaOperation {
    /// Every operation, in the order a delta file conventionally writes them.
    ///
    /// A `const` and not a `Vec`: the one caller that needs it — reading an archived change,
    /// where there is no CLI answer saying which sections a file uses — scans all four, and a
    /// variant added without a row here would be a section silently never looked for.
    pub const EVERY: [Self; 4] = [Self::Added, Self::Modified, Self::Removed, Self::Renamed];

    /// The `## <HEADER> Requirements` word this operation is written as in a delta file.
    ///
    /// Uppercase and not a `Display` impl: this is the *file's* spelling, which the block
    /// scanner matches on and the writer emits, and it must not drift into being whatever a
    /// panel happens to want to print.
    pub fn header(self) -> &'static str {
        match self {
            Self::Added => "ADDED",
            Self::Modified => "MODIFIED",
            Self::Removed => "REMOVED",
            Self::Renamed => "RENAMED",
        }
    }
}

/// A rename, `FROM:`/`TO:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecRename {
    pub from: String,
    pub to: String,
}

/// One requirement: its text, and the scenarios that make it testable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecRequirement {
    /// The name off the `### Requirement: <name>` header, which is what the archive matches on.
    ///
    /// **Read from the delta file, not from the CLI's JSON**, which does not carry it: a
    /// requirement's `text` there has already had its header folded away. See
    /// `cide_spec::block::parse` for the whole argument — the short version is that the name is
    /// how an edit addresses a block, so it has to come from the same scanner the writer uses.
    pub name: String,
    /// The requirement's prose — everything between the header and the first scenario, as
    /// written.
    pub text: String,
    pub scenarios: Vec<SpecScenario>,
    /// The whole block, header included, exactly as it is in the file.
    ///
    /// This is what the editor edits and what `spec_requirement_set` replaces. Carried verbatim
    /// rather than recomposed from the fields above, because recomposing is lossy in a way
    /// nobody would notice until a diff: a requirement whose prose the user never touched would
    /// come back reflowed, and the blank lines, the indentation and any markup the fields do not
    /// model would be silently normalised away.
    pub block: String,
}

/// One `#### Scenario:` section of a requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecScenario {
    /// The title off its `#### Scenario: <title>` header.
    ///
    /// Also absent from the CLI's JSON — a scenario's `rawText` is its body alone — and also
    /// recovered from the file. A card that showed scenarios with no titles would be showing a
    /// reviewer a list of WHEN/THEN bullets with nothing saying what each one is a case *of*.
    pub title: String,
    /// Everything under the header, as written.
    pub body: String,
}

/// One of a change's artifacts, and where it actually is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecArtifact {
    /// The schema's id for it — `proposal`, `design`, `tasks`, `specs`.
    pub id: String,
    /// What the schema says it generates, which may be a glob.
    pub generates: String,
    pub state: ArtifactState,
    /// The files that exist for it, absolute.
    ///
    /// **This is the only way a filename is learned.** The artifact set is schema-driven, so
    /// `proposal.md` is a default and not a guarantee; anything that hard-coded it would break
    /// on the first project with a custom schema, and break by looking at the wrong file rather
    /// than by failing.
    #[ts(type = "string[]")]
    pub existing: Vec<PathBuf>,
}

/// How far one artifact has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ArtifactState {
    Done,
    Skipped,
    Ready,
    Blocked,
}

/// A change's checklist, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecProgress {
    pub tasks: Vec<SpecTask>,
    pub total: u32,
    pub completed: u32,
}

impl SpecProgress {
    /// Is every step ticked, with at least one step to tick?
    ///
    /// The `total > 0` half is the whole of it: a change whose tracked file exists and holds no
    /// checkboxes reports `0/0`, and reading that as "finished" would move a task to `Review`
    /// the moment somebody created an empty `tasks.md`.
    pub fn complete(&self) -> bool {
        self.total > 0 && self.completed == self.total
    }
}

/// One `- [ ]` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecTask {
    pub done: bool,
    pub description: String,
}

/// What `openspec validate --strict` said.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecValidation {
    pub valid: bool,
    pub issues: Vec<SpecIssue>,
}

/// One validation complaint, in the shape the Problems panel already draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecIssue {
    /// `ERROR`, `WARNING`, `INFO` — upstream's spelling, for [`ChangeSummary::status`]'s reason.
    pub level: String,
    /// Where in the document, as the validator names it.
    pub path: String,
    pub message: String,
    #[ts(optional)]
    pub line: Option<u32>,
    #[ts(optional)]
    pub column: Option<u32>,
}

impl SpecIssue {
    /// Is this one of the complaints that makes a change invalid?
    ///
    /// Case-insensitive on purpose: the level is upstream's string, and a comparison that
    /// assumed its case would silently classify every issue as non-blocking the day it changed.
    pub fn blocking(&self) -> bool {
        self.level.eq_ignore_ascii_case("error")
    }
}

/// One artifact file's text, and whether it is all of it.
///
/// `truncated` is carried rather than inferred so the section can say so. A panel that showed
/// three quarters of a design document with nothing on screen saying it had stopped would be a
/// silent lie of exactly the kind this codebase keeps writing guards against — and the reader
/// would blame the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecArtifactText {
    pub text: String,
    pub truncated: bool,
}

/// Set one requirement block in one delta file. (M28)
///
/// Inbound, so `deny_unknown_fields`: a field the frontend sends that Rust has since renamed must
/// be a refusal the developer sees, not a value silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct SpecRequirementSet {
    pub project: ProjectId,
    pub change: ChangeName,
    pub spec: SpecId,
    pub operation: DeltaOperation,
    /// The requirement to replace, by the name on its header.
    pub requirement: String,
    /// The whole replacement block, `### Requirement:` header included.
    pub block: String,
}

/// What writing a requirement block did.
///
/// Three arms rather than `Result<(), String>`, because two of the three failures are *states the
/// panel draws differently*: a regression has issues to show against the fields that caused them,
/// and a conflict has a file somebody else is holding and a Reload to offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecWriteOutcome {
    Written {
        change: SpecChange,
        #[ts(type = "string")]
        path: PathBuf,
    },
    /// The write validated worse than what was there, and has been rolled back.
    Regressed { issues: Vec<SpecIssue> },
    /// The file changed between the read and the write — an agent holds it. Nothing was written.
    Conflicted {
        #[ts(type = "string")]
        path: PathBuf,
    },
}

/// What accepting a change would do, before anybody presses anything. (M28)
///
/// # Why a preview exists at all
///
/// Because the gesture behind it is two irreversible things in a row — a merge into the branch
/// the user has checked out, and a rewrite of `openspec/specs/`, which is what every later run
/// reads as ground truth. `ConfirmDestructive`'s rule is that a *count* is not enough: the user
/// is about to act on specific files and "3 requirements" is not something anyone can check. So
/// this names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecAcceptPlan {
    pub change: ChangeName,
    /// The branch that would be merged — `cide/<role>-<task>` — or `None` for a role that runs
    /// in the checked-out tree (`worktree: false`), where there is nothing to merge.
    ///
    /// `None` is **not** a refusal, and the preview has to say so plainly, or a user will read a
    /// missing step as a step that was silently skipped.
    #[ts(optional)]
    pub branch: Option<String>,
    pub completed_tasks: u32,
    pub total_tasks: u32,
    /// The capability files `archive` would rewrite, and what it would do to each.
    pub specs_touched: Vec<SpecTouch>,
    /// Empty means the gesture will go through. Non-empty is the list of sentences to draw
    /// instead of the button, each naming what to do next.
    pub refusals: Vec<String>,
}

/// One capability an archive would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecTouch {
    pub spec: SpecId,
    pub operation: DeltaOperation,
    pub requirements: u32,
}

/// What accepting actually did.
///
/// Four arms, and three of them are ordinary outcomes rather than errors — which is the point:
/// a conflict is a list of paths to go and look at, and a refusal is a sentence with a next
/// action in it. Only a genuinely broken call is an `Err`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecAccepted {
    /// Merged and archived, and the task is `done`.
    Accepted {
        /// `None` when there was no branch to merge — a `worktree: false` role.
        #[ts(optional)]
        commit: Option<String>,
        files: u32,
        change: ChangeName,
    },
    /// The plan said no. Nothing was merged and nothing was archived.
    Refused { plan: SpecAcceptPlan },
    /// The merge refused, so **nothing was archived**.
    ///
    /// The order is the whole reason this gesture is one button: archiving after a refused merge
    /// would put behaviour into `openspec/specs/` that the checked-out branch does not implement,
    /// and every later run would read it as true.
    Conflicts { paths: Vec<String> },
}

/* ==============================================================================================
 * `openspec/config.yaml` — the project's own configuration, as a form. (M28)
 *
 * These four types are the wire for the *config* surface, and they sit in this module rather than
 * in `crate::workspace` for the reason the module header gives: `openspec/` is somebody else's
 * format, and the boundary that keeps an upstream change out of the panels has to hold for the
 * configuration file as much as for the board.
 *
 * They mirror `cide_spec::config`'s domain types rather than re-exporting them, which is the same
 * split `SpecBoard` makes against `cide_spec::model` and for one extra reason: `cide-spec` must
 * not carry `ts_rs`, and the *form* needs three things the reader has no use for — where the file
 * is, whether it is there at all, and what an unstated `schema:` resolves to.
 * ============================================================================================ */

/// A project's OpenSpec configuration, as far as cide's reader understands it.
///
/// # Why nothing here is filled in with a default
///
/// `schema` stays `None` when the file states no schema, exactly as `cide_spec::config::Config`
/// keeps it — and [`Self::default_schema`] is carried *beside* it rather than substituted into
/// it. A form that showed `spec-driven` in both cases could not tell "unstated" from "stated,
/// and happens to be the default", and pressing Save would then add a line to a committed file
/// that nobody asked for. The round-trip guarantee `cide_spec::config` makes — writing a value
/// back unchanged changes no byte — only survives if the value being written back is the one the
/// file actually states.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecConfig {
    /// The workflow schema id the file states, or `null` when it states none.
    pub schema: Option<String>,
    /// What an unstated `schema:` means — `cide_spec::config::DEFAULT_SCHEMA`.
    ///
    /// On the wire rather than typed into TypeScript because it is the *CLI's* default, not
    /// cide's, and a second copy in the frontend is one that goes stale in silence the day
    /// OpenSpec ships a different one. The select shows it as the resolved value.
    pub default_schema: String,
    /// Free prose injected into every artifact-generation prompt, newlines and all.
    ///
    /// The high-value field: everything an agent is told about this project's stack and
    /// conventions when it writes a proposal, a design or a spec comes from here.
    pub context: Option<String>,
    /// Per-artifact rules, in the order the file states them.
    ///
    /// Ordered `Vec`s and not maps, for [`Self::operations`] too: document order is what the file
    /// says and what the form should show. A map would alphabetise a list the user chose the
    /// order of, and the write path splices lines back where it found them.
    pub rules: Vec<SpecArtifactRules>,
    /// Per-operation guidance, in the order the file states them.
    pub operations: Vec<SpecOperationGuidance>,
    /// `<root>/openspec/config.yaml`, absolute.
    ///
    /// Carried because the form deliberately shows **less** than the file holds. `openspec init`
    /// writes 922 bytes of which roughly eight hundred are comments, and those comments are the
    /// only documentation `context`, `rules` and `operations` have anywhere. A form that hid them
    /// with no way through to the file would be hiding instructions, so the surface links to this
    /// path and opens it in an editor tab.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Whether that file exists.
    ///
    /// A missing file reads as an empty config and is deliberately **not** an error — see
    /// `cide_spec::config::read` — so nothing else in this struct can distinguish "there is no
    /// config" from "there is a config that states nothing", and those two want different words
    /// above the same form.
    pub exists: bool,
}

/// One artifact's rules.
///
/// `artifact` is a free string and not an enum, and that is load-bearing: a workflow schema
/// declares its own artifacts with `generates` globs, so `rules:` may perfectly legally name one
/// cide has never heard of. `cide_spec::config::DEFAULT_ARTIFACTS` exists so a form has something
/// to *offer*; neither the reader nor the writer consults it, and neither does this.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecArtifactRules {
    pub artifact: String,
    pub rules: Vec<String>,
}

/// One operation's guidance — `apply` or `archive`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecOperationGuidance {
    pub operation: String,
    pub guidance: Vec<String>,
}

/// One change to make to `openspec/config.yaml`.
///
/// The wire twin of `cide_spec::config::Edit`, and a `Vec` of these is what one Save sends. Four
/// separate commands would be four reads, four validations and four atomic renames of one
/// committed file for a single gesture — four chances to lose a race with an agent holding the
/// same file, and up to four commits' worth of mtime churn. `config::apply` takes the whole slice
/// and writes once, or, when every value already reads that way, not at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum SpecConfigEdit {
    /// Set `schema:`.
    Schema { schema: String },
    /// Set `context:`, or remove the key entirely with `null`.
    Context { context: Option<String> },
    /// Set one artifact's rules. An empty list removes that artifact's entry, and removing the
    /// last one removes `rules:` with it — a bare `rules:` is YAML `null`, not an empty mapping.
    Rules {
        artifact: String,
        rules: Vec<String>,
    },
    /// Set one operation's guidance. Empty removes it, by the same rule.
    OperationGuidance {
        operation: String,
        guidance: Vec<String>,
    },
}

/// One workflow schema the CLI offers.
///
/// # Why this is not `Vec<String>`
///
/// Because `openspec schemas --json` answers with the artifacts each schema declares, and the
/// artifact list is exactly the thing cide is forbidden to assume. `cide_spec::config`'s
/// `DEFAULT_ARTIFACTS` is documented as "a default and **not** a closed set — the schema decides",
/// so a rules editor that offered four hard-coded rows would be offering the wrong ones for any
/// project on a schema it did not ship with, with nothing on screen to say so. Taking the
/// artifacts from the same document that names the schema is the only version of this that stays
/// true when somebody installs a custom schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpecSchema {
    /// The id, as `schema:` would spell it.
    pub name: String,
    /// One line, the CLI's own words, or `null` when it gave none.
    pub description: Option<String>,
    /// The artifacts this schema generates, in workflow order.
    pub artifacts: Vec<String>,
    /// `package`, `project`, … — where the schema came from, in the CLI's spelling.
    ///
    /// Upstream's word rather than an enum, for [`ChangeSummary::status`]'s reason: it is shown
    /// and never branched on, and a fifth source arriving must not be a parse failure.
    pub source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(level: &str) -> SpecIssue {
        SpecIssue {
            level: level.to_string(),
            path: "requirement".to_string(),
            message: "needs a scenario".to_string(),
            line: Some(4),
            column: None,
        }
    }

    #[test]
    fn a_spec_board_names_its_three_states_apart() {
        // The tag is what the frontend switches on, and each arm is a different screen.
        let absent = SpecBoard::Absent {
            hint: "No OpenSpec in this project.".into(),
            path: PathBuf::from("/repo/openspec"),
        };
        let json = serde_json::to_string(&absent).expect("serialises");
        assert!(json.contains("\"kind\":\"absent\""), "{json}");
        assert!(
            json.contains("/repo/openspec"),
            "the directory that would be created is named: {json}"
        );

        let unusable = SpecBoard::Unusable {
            reason: "`openspec` is not on this app's PATH".into(),
        };
        assert!(
            serde_json::to_string(&unusable)
                .unwrap()
                .contains("\"kind\":\"unusable\"")
        );
    }

    #[test]
    fn the_dtos_round_trip_under_camel_case() {
        let change = SpecChange {
            name: ChangeName("add-dark-mode".into()),
            title: "add-dark-mode".into(),
            deltas: vec![SpecDelta {
                spec: SpecId("dark-mode".into()),
                operation: DeltaOperation::Modified,
                description: "the switch moves per-window".into(),
                requirements: vec![SpecRequirement {
                    name: "Theme switching".into(),
                    text: "The app SHALL switch themes.".into(),
                    scenarios: vec![SpecScenario {
                        title: "toggled".into(),
                        body: "- **WHEN** …".into(),
                    }],
                    block: "### Requirement: Theme switching".into(),
                }],
                rename: None,
            }],
            artifacts: vec![SpecArtifact {
                id: "tasks".into(),
                generates: "tasks.md".into(),
                state: ArtifactState::Ready,
                existing: vec![PathBuf::from(
                    "/repo/openspec/changes/add-dark-mode/tasks.md",
                )],
            }],
            progress: SpecProgress {
                tasks: vec![SpecTask {
                    done: true,
                    description: "add the tokens".into(),
                }],
                total: 1,
                completed: 1,
            },
            validation: SpecValidation {
                valid: true,
                issues: vec![],
            },
            origin: SpecOrigin::Active,
        };
        let wire = serde_json::to_string(&change).expect("serialises");
        assert!(
            wire.contains("\"completedTasks\"") || wire.contains("\"existing\""),
            "{wire}"
        );
        assert!(wire.contains("\"operation\":\"modified\""), "{wire}");
        assert_eq!(
            serde_json::from_str::<SpecChange>(&wire).expect("deserialises"),
            change
        );
        assert!(wire.contains("\"origin\":{\"kind\":\"active\"}"), "{wire}");

        // The archived arm carries the directory's own name, stamp and all — the only place it
        // is recoverable, and what a reader needs to find the files.
        let archived = SpecChange {
            origin: SpecOrigin::Archived {
                folder: "2026-08-27-add-dark-mode".into(),
            },
            ..change
        };
        let wire = serde_json::to_string(&archived).expect("serialises");
        assert!(
            wire.contains("\"folder\":\"2026-08-27-add-dark-mode\""),
            "{wire}"
        );
        assert_eq!(
            serde_json::from_str::<SpecChange>(&wire).expect("deserialises"),
            archived
        );
    }

    #[test]
    fn an_inbound_request_refuses_a_field_it_does_not_know() {
        // `deny_unknown_fields`, so a rename on either side is a refusal a developer sees rather
        // than a value that quietly stopped arriving.
        // Built rather than written as a raw string: the value starts `### `, so every
        // `r#`-family delimiter is closed early by the `"#` the JSON itself contains.
        let json = format!(
            "{{\"project\":\"00000000-0000-4000-8000-000000000000\",\
             \"change\":\"add-dark-mode\",\"spec\":\"dark-mode\",\
             \"operation\":\"added\",\"requirement\":\"Theme switching\",\
             \"block\":\"{}\",\"extra\":true}}",
            "### Requirement: Theme switching"
        );
        assert!(serde_json::from_str::<SpecRequirementSet>(&json).is_err());
    }

    #[test]
    fn a_checklist_with_no_tasks_is_not_a_finished_one() {
        // The Review hop keys off exactly this, and `0/0` reading as complete would move a task
        // the moment somebody created an empty tasks.md.
        let empty = SpecProgress {
            tasks: vec![],
            total: 0,
            completed: 0,
        };
        assert!(!empty.complete());

        let done = SpecProgress {
            tasks: vec![],
            total: 3,
            completed: 3,
        };
        assert!(done.complete());

        let partial = SpecProgress {
            tasks: vec![],
            total: 3,
            completed: 2,
        };
        assert!(!partial.complete());
    }

    #[test]
    fn only_errors_block_and_the_level_is_matched_whatever_its_case() {
        assert!(issue("ERROR").blocking());
        assert!(issue("error").blocking());
        assert!(!issue("WARNING").blocking());
        assert!(!issue("INFO").blocking());
    }

    #[test]
    fn a_delta_operation_writes_the_header_the_file_uses() {
        // The file's spelling, not a panel's — the scanner matches on it and the writer emits it.
        assert_eq!(DeltaOperation::Added.header(), "ADDED");
        assert_eq!(DeltaOperation::Renamed.header(), "RENAMED");
        // …and the wire spelling is the frontend's, which is a different string on purpose.
        assert_eq!(
            serde_json::to_string(&DeltaOperation::Added).unwrap(),
            "\"added\""
        );
    }
}
