//! Reading a project's `openspec/` — by running the `openspec` CLI, never by parsing it. (M28)
//!
//! [OpenSpec](https://github.com/Fission-AI/OpenSpec) is a tool-agnostic convention for
//! spec-driven development: requirements as they stand in `openspec/specs/<capability>/spec.md`,
//! and each piece of proposed work as `openspec/changes/<name>/` — a proposal, a design, a
//! `tasks.md` checklist, and delta files stating `## ADDED | MODIFIED | REMOVED | RENAMED
//! Requirements`. Archiving a change merges its deltas into `specs/`.
//!
//! # Why nothing here parses markdown
//!
//! It was the first design, and reading the shipped CLI killed it. Its grammar carries
//! code-fence masking, case-folded requirement-name matching for typo detection, `RENAMED`
//! `FROM:`/`TO:` sections, dropped-scenario detection on `MODIFIED`, and — the one that settles
//! it — a **schema-driven artifact set**: `openspec/config.yaml` selects a workflow schema whose
//! artifacts are declared with `generates` globs, so even `tasks.md` is not a filename cide may
//! assume. A Rust reimplementation would be a second parser that agrees with the first until the
//! next `npm i -g`, and every way it could disagree is silent: a checkbox not counted, a
//! requirement not found, a board that looks right and is not.
//!
//! So this crate is an **adapter**. It locates the binary ([`discover`]), runs it with a deadline
//! and a null stdin ([`cli`]), and maps its `--json` into `cide_ipc::spec`'s types. The one place
//! it does read the files itself is [`block`], and that is not a parser but a *byte-range
//! scanner* mirroring the CLI's own — see its header.
//!
//! # Three facts about the CLI that the code turns on
//!
//! **The exit code is not the failure signal.** Every `--json` command exits 0 and reports
//! failure as a `status` array on stdout. A caller that checked `ok` would treat
//! `openspec show nope --json` as success with an empty document.
//!
//! **Root resolution walks ancestors.** `openspec` climbs parent directories looking for an
//! `openspec/`, and reports what it found as `root: {path, source}`. A worktree that has none
//! therefore silently answers with *the parent repository's* board — which, since a live agent's
//! progress is read from its worktree, would report the wrong checklist as that run's. Every
//! call pins the resolved root against the directory it asked about.
//!
//! **A bare invocation prompts.** `openspec init` asks which of forty AI tools to configure and
//! plays an animation. `OPEN_SPEC_INTERACTIVE=0` and a null stdin close that off twice over,
//! because a prompt on a command worker is not a failure — it is a hang.
//!
//! # What this crate deliberately does not do
//!
//! No broadcast, no `AppHandle`, no thread, no cache. `cide_agents::load_project`'s posture: a
//! board is a re-read, and the coalescing that keeps re-reads from being a storm belongs to the
//! layer that owns the watcher. It also never runs `archive` as a side effect of anything —
//! that is the one command that rewrites files cide did not open, and it stays a gesture.

pub mod block;
pub mod claude;
pub mod cli;
pub mod config;
pub mod discover;
pub mod model;
pub mod write;

use std::path::{Path, PathBuf};

use cide_ipc::{ChangeName, SpecBoard};

/// The directory OpenSpec lives in, relative to a project root.
pub const SPEC_RELATIVE: &str = "openspec";

/// The directory `openspec archive` moves a finished change into, under `openspec/changes/`.
///
/// Named here because cide reads it directly — no CLI command will, see [`archived_change`].
pub const ARCHIVE_DIR: &str = "archive";

/// `<root>/openspec`.
pub fn spec_path(root: &Path) -> PathBuf {
    root.join(SPEC_RELATIVE)
}

/// Does this directory use OpenSpec?
///
/// **cide's own answer, and it has to be.** `openspec list --json` in a directory with no
/// `openspec/` exits 0 with an empty list and `root.source: "implicit"` — identical, to a caller
/// reading the exit code, to a project that is set up and has proposed nothing. And because the
/// CLI walks ancestors, a directory nested inside a repository that *does* have one would report
/// the parent's board as its own.
pub fn present(root: &Path) -> bool {
    spec_path(root).is_dir()
}

/// A located `openspec`, bound to the directory it will be run in.
///
/// `cwd` is a field rather than a per-call argument because it is the single most dangerous
/// thing to get wrong here: while an agent is working, its checklist lives in
/// `.cide/worktrees/<agent>/openspec/`, and a call made from the project root would answer with
/// the *project's* progress and look entirely plausible doing it.
#[derive(Debug, Clone)]
pub struct Openspec {
    binary: PathBuf,
    cwd: PathBuf,
}

/// Why a call produced nothing usable.
///
/// Tagged rather than prose, `FilterError`'s reason: the panel, the headless renderer and the
/// Review hop each phrase these differently, and a `String` here would be one crate's wording
/// leaking into three.
#[derive(Debug)]
pub enum SpecError {
    /// No binary. Carries the ladder's own sentence.
    NoBinary(String),
    /// It could not be run, or did not finish.
    Ran(String),
    /// It ran and its output was not the JSON this build expects.
    Unreadable { what: String, detail: String },
    /// It ran and refused — the `status` array, rendered.
    Refused(String),
    /// It resolved a root that is not the directory we asked about. See the crate header.
    NoRoot { asked: PathBuf, resolved: String },
    /// Something in the write path. Carries its own sentence.
    Write(String),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoBinary(sentence) | Self::Ran(sentence) | Self::Write(sentence) => {
                f.write_str(sentence)
            }
            Self::Refused(sentence) => f.write_str(sentence),
            Self::Unreadable { what, detail } => write!(
                f,
                "cide could not read what `openspec {what}` answered — this build may be older \
                 than the installed CLI ({detail})"
            ),
            Self::NoRoot { asked, resolved } => write!(
                f,
                "`openspec` resolved its root to {resolved} rather than to {}. OpenSpec searches \
                 parent directories, so this would have reported another project's specs.",
                asked.display()
            ),
        }
    }
}

impl std::error::Error for SpecError {}

impl Openspec {
    /// Locate the binary and bind it to a directory.
    pub fn open(cwd: &Path) -> Result<Self, SpecError> {
        let binary = discover::find().map_err(|refusal| SpecError::NoBinary(refusal.sentence()))?;
        Ok(Self {
            binary,
            cwd: cwd.to_path_buf(),
        })
    }

    /// The directory this instance runs in — the project root, or one agent's worktree.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The resolved binary.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// The board: active changes and the capabilities that already exist.
    /// Both halves run at once, for [`Self::change`]'s reason: each is a node process and a
    /// panel that waited for them in turn waited twice as long as it had to.
    pub fn board(&self) -> Result<SpecBoard, SpecError> {
        let (changes, specs) = std::thread::scope(|scope| {
            let changes = scope.spawn(|| -> Result<model::ListChanges, SpecError> {
                self.json(&["list", "--json", "--sort", "recent"])
            });
            let specs = scope.spawn(|| -> Result<model::ListSpecs, SpecError> {
                self.json(&["list", "--specs", "--json"])
            });
            (
                changes
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
                specs
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
            )
        });
        let changes = changes?;
        // Pinned before the second answer is looked at, so a root that walked out of this
        // directory is refused whatever the other thread came back with.
        self.pin_root(changes.root.as_ref(), "list")?;
        let specs = specs?;
        Ok(SpecBoard::Ready {
            changes: changes.changes.into_iter().map(Into::into).collect(),
            specs: specs.specs.into_iter().map(Into::into).collect(),
            root: self.cwd.clone(),
            // Read from the directory rather than assumed, and carried so the panel previews the
            // line it is about to send. Two `stat`s per surface against two subprocesses already
            // in flight — see `claude` for what happens when this is a table instead.
            commands: claude::installed(&self.cwd),
        })
    }

    /// One change in full: its deltas, its artifacts, its checklist and its verdict.
    ///
    /// # The four reads run at once, and that is a user-visible fact
    ///
    /// A change page is four `openspec` invocations — `show`, `status`, `instructions apply` and
    /// `validate` — and each is a *node process*: measured against a real project, every one of
    /// them costs about 0.55s almost all of which is the interpreter starting. Run in sequence
    /// that is a page that says "Reading…" for well over two seconds, every time the board moves,
    /// which while an agent is ticking boxes is often.
    ///
    /// They are independent — four reads of the same directory, no shared state, nothing written
    /// — so they are four threads and the page costs one process instead of four. `Openspec` is
    /// two `PathBuf`s and every call takes `&self`, so a scope is all this needs; nothing here is
    /// shared mutably and there is no runtime to reach for.
    ///
    /// The **order of the error** is pinned rather than "whichever thread failed first": a race
    /// deciding which sentence the panel shows would make one refusal intermittently become
    /// another, and that is the kind of bug that gets reported as "it says something different
    /// every time".
    pub fn change(&self, change: &ChangeName) -> Result<cide_ipc::SpecChange, SpecError> {
        let (shown, status, progress, validation) = std::thread::scope(|scope| {
            let shown = scope.spawn(|| -> Result<model::ShowChange, SpecError> {
                self.json(&["show", change.as_str(), "--json", "--type", "change"])
            });
            let status = scope.spawn(|| -> Result<model::Status, SpecError> {
                self.json(&["status", "--change", change.as_str(), "--json"])
            });
            let progress = scope.spawn(|| self.progress(change));
            let validation = scope.spawn(|| self.validate(Some(change)));
            // A panicking thread here would be a bug in this crate, not a state to model, and
            // `join`'s `Err` carries the payload of one that already printed its own backtrace.
            // Resuming it keeps the panic a panic instead of turning it into a refusal sentence
            // that blames the CLI for something cide did.
            (
                shown
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
                status
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
                progress
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
                validation
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload)),
            )
        });
        let shown = shown?;
        let status = status?;
        let progress = progress?;
        let validation = validation?;
        let artifacts = status.into_artifacts();
        let deltas = group_deltas(shown.deltas, &artifacts);
        Ok(cide_ipc::SpecChange {
            title: shown
                .title
                .or(shown.id)
                .unwrap_or_else(|| change.as_str().to_string()),
            name: change.clone(),
            deltas,
            artifacts,
            progress,
            validation,
            origin: cide_ipc::SpecOrigin::Active,
        })
    }

    /// The checklist, from the CLI's own resolution of it.
    ///
    /// `instructions apply` and not a read of `tasks.md`: which file *is* the checklist is a
    /// schema question, the counting rule tolerates nested and indented items, and both live
    /// upstream. See the crate header.
    pub fn progress(&self, change: &ChangeName) -> Result<cide_ipc::SpecProgress, SpecError> {
        let answer: model::ApplyInstructions = self.json(&[
            "instructions",
            "apply",
            "--change",
            change.as_str(),
            "--json",
        ])?;
        Ok(answer.into())
    }

    /// Validate one change, or everything.
    pub fn validate(
        &self,
        change: Option<&ChangeName>,
    ) -> Result<cide_ipc::SpecValidation, SpecError> {
        let mut args = vec!["validate"];
        match change {
            Some(change) => args.push(change.as_str()),
            None => args.push("--all"),
        }
        args.extend(["--json", "--strict"]);
        let answer: model::Validation = self.json_with(&args, cli::VALIDATE)?;
        Ok(answer.into())
    }

    /// One of a change's artifact files, as text.
    ///
    /// # Why a separate call, and why a cap
    ///
    /// `show --json` carries a change's deltas and **none of its prose** — the proposal and the
    /// design are files. They are read here, on demand, rather than folded into
    /// [`Self::change`], because the panel draws those sections collapsed: fetching a document
    /// nobody has expanded would be a file read on every board refresh for prose nobody is
    /// looking at.
    ///
    /// The cap is not about memory, it is about the wire: this value crosses the IPC boundary as
    /// JSON and lands in a webview. A `design.md` somebody pasted a heap profile into would
    /// otherwise freeze the panel that opened it. Truncation is *reported*, so the section can
    /// say so and offer the file — silently showing three quarters of a document is the failure
    /// this cap must not become.
    pub fn artifact(&self, path: &Path) -> Result<ArtifactText, SpecError> {
        const CAP: usize = 256 * 1024;
        // The jail: an artifact path came from the CLI, but it reaches here through the frontend,
        // and a path outside this project's `openspec/` is not something cide reads on a
        // panel's say-so.
        let root = spec_path(&self.cwd);
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !canonical.starts_with(&root_canonical) {
            return Err(SpecError::Write(format!(
                "{} is not inside {}",
                path.display(),
                root.display()
            )));
        }
        let bytes = std::fs::read(&canonical).map_err(|error| {
            SpecError::Write(format!("{} could not be read: {error}", path.display()))
        })?;
        let truncated = bytes.len() > CAP;
        let slice = if truncated { &bytes[..CAP] } else { &bytes[..] };
        // Lossy, and only here: this value is *shown*, never written back. The write path reads
        // its own bytes and refuses non-UTF-8 rather than mangling it.
        Ok(ArtifactText {
            text: String::from_utf8_lossy(slice).to_string(),
            truncated,
        })
    }

    /// Set up OpenSpec in this directory.
    ///
    /// `--tools claude` and never `--tools none`: `init` is what installs OpenSpec's workflow
    /// into the project's own Claude Code — as skills under `.claude/skills/` on a current CLI,
    /// as slash commands under `.claude/commands/opsx/` on an older one — and that workflow is
    /// the prose cide deliberately does not write for itself. A `none` here would leave the
    /// pinned session with nothing to offer and cide paraphrasing upstream's instructions
    /// instead. Which surface a project ended up with is [`claude`]'s question, not this one's.
    pub fn init(&self) -> Result<(), SpecError> {
        self.run(
            &[
                "init",
                ".",
                "--tools",
                "claude",
                "--force",
                "--no-animation",
            ],
            cli::MUTATE,
        )?;
        Ok(())
    }

    /// Scaffold a new change directory from the schema's templates.
    pub fn new_change(
        &self,
        name: &ChangeName,
        description: Option<&str>,
    ) -> Result<(), SpecError> {
        let mut args = vec!["new", "change", name.as_str(), "--json"];
        if let Some(description) = description {
            args.extend(["--description", description]);
        }
        self.run(&args, cli::MUTATE)?;
        Ok(())
    }

    /// Scaffold a change **and give it a proposal**, from a title and a body.
    ///
    /// # Why this exists beside [`Self::new_change`]
    ///
    /// Because `new change` alone produces a change cide cannot display. It writes `README.md`
    /// and `.openspec.yaml` and nothing else — the proposal is `propose`'s job, i.e. an
    /// agent's — and `openspec show <change> --json` **refuses** a change with no `proposal.md`:
    /// *"Change X has no proposal.md yet."* So a panel that offered "new change from this task"
    /// and called `new_change` would create a board row that opens onto an error, and the user
    /// would have no way to tell whether they or cide had done something wrong.
    ///
    /// The proposal written here is a *stub with the user's own words in it*, not a finished
    /// document: `Why` from the task's body, `What Changes` naming the title. It is the thing an
    /// agent is then asked to flesh out, and it is immediately readable in the panel, which is
    /// what makes the next gesture possible.
    ///
    /// The `Why` section has a 50-character floor upstream (`MIN_WHY_SECTION_LENGTH`), so a body
    /// shorter than that is padded with the title rather than being written into a document the
    /// validator will refuse for a reason the user cannot see from the panel.
    pub fn propose(&self, name: &ChangeName, title: &str, body: &str) -> Result<(), SpecError> {
        self.new_change(name, Some(title))?;
        let path = spec_path(&self.cwd)
            .join("changes")
            .join(name.as_str())
            .join("proposal.md");
        if path.exists() {
            // A schema whose template already writes one. Leave it: it is upstream's shape, and
            // overwriting it would be cide deciding it knows the schema better.
            return Ok(());
        }
        let why = proposal_why(title, body);
        // Assembled line by line rather than as one `format!`. A multi-line string literal
        // carries the *source's* own indentation into its value, so the continuation lined up
        // with the opening quote put a run of spaces in front of `## Capabilities` and turned a
        // heading into an indented paragraph. That is what the first version of this shipped, and
        // the real CLI reported it back as a proposal with no sections at all.
        let h = "##";
        let text = [
            format!("{h} Why"),
            String::new(),
            why,
            String::new(),
            format!("{h} What Changes"),
            String::new(),
            format!("- {}", title.trim()),
            String::new(),
            format!("{h} Capabilities"),
            String::new(),
            format!("{h}# New Capabilities"),
            String::new(),
            format!("{h} Impact"),
            String::new(),
        ]
        .join("\n");
        cide_core::persist::write_atomic_with_mode(
            &path,
            text.as_bytes(),
            cide_core::persist::SHARED_MODE,
        )
        .map_err(|error| {
            SpecError::Write(format!("{} could not be written: {error}", path.display()))
        })?;
        Ok(())
    }

    /// Merge a finished change's deltas into `specs/` and move it to the archive.
    ///
    /// Never `--skip-specs` and never `--no-validate`. Both are real answers a person may want,
    /// and both are answers cide must not give on their behalf: the first writes an archive that
    /// changes no requirement, the second files behaviour into the source of truth that the
    /// validator was not allowed to look at. A user who means either can say so in a terminal.
    pub fn archive(&self, change: &ChangeName) -> Result<(), SpecError> {
        self.run(
            &["archive", change.as_str(), "--yes", "--json"],
            cli::MUTATE,
        )?;
        Ok(())
    }

    /// Run and parse, at the read deadline.
    fn json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T, SpecError> {
        self.json_with(args, cli::READ)
    }

    fn json_with<T: serde::de::DeserializeOwned>(
        &self,
        args: &[&str],
        deadline: std::time::Duration,
    ) -> Result<T, SpecError> {
        let stdout = self.run(args, deadline)?;
        cli::parse(&stdout, &args.join(" "))
    }

    fn run(&self, args: &[&str], deadline: std::time::Duration) -> Result<Vec<u8>, SpecError> {
        cli::run(&self.binary, &self.cwd, args, deadline)
    }

    /// Refuse an answer that describes a different directory. See the crate header.
    fn pin_root(&self, root: Option<&model::Root>, what: &str) -> Result<(), SpecError> {
        let asked = self.cwd.canonicalize().unwrap_or_else(|_| self.cwd.clone());
        match root {
            Some(root) if root.source.as_deref() == Some("implicit") => Err(SpecError::NoRoot {
                asked,
                resolved: "nothing (no openspec/ was found from here)".to_string(),
            }),
            Some(root) => {
                let resolved = PathBuf::from(&root.path);
                let resolved = resolved.canonicalize().unwrap_or(resolved);
                if resolved == asked {
                    Ok(())
                } else {
                    Err(SpecError::NoRoot {
                        asked,
                        resolved: resolved.display().to_string(),
                    })
                }
            }
            // A build of the CLI that stopped reporting a root at all. Refusing beats trusting:
            // the whole reason this check exists is that the wrong answer looks right.
            None => Err(SpecError::Unreadable {
                what: what.to_string(),
                detail: "it reported no root".to_string(),
            }),
        }
    }
}

/// One artifact file's text, and whether it is all of it.
///
/// `truncated` is carried rather than inferred so the section can say so. A panel that showed
/// three quarters of a design document with nothing on screen saying it had stopped would be a
/// silent lie of exactly the kind this codebase keeps writing guards against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactText {
    pub text: String,
    pub truncated: bool,
}

/// The `Why` section of a stub proposal, from a task's title and body.
///
/// Upstream refuses a `Why` under 50 characters, and the refusal names a length rather than a
/// document — which from the panel would read as cide having written something malformed. So a
/// short body is *extended* with the title and a sentence saying the proposal is a stub, which is
/// both true and long enough. An empty body is the common case: somebody typed a title and
/// pressed Create.
fn proposal_why(title: &str, body: &str) -> String {
    const FLOOR: usize = 50;
    let body = body.trim();
    if body.chars().count() >= FLOOR {
        return body.to_string();
    }
    let stub = format!(
        "{}{}This is a stub proposal for `{}` — say here what problem it solves and why now.",
        body,
        if body.is_empty() { "" } else { "\n\n" },
        title.trim()
    );
    stub
}

/// The CLI's delta rows, grouped into one delta per `(capability, operation)`. (M28)
///
/// # The CLI emits one row per *requirement*, not per file
///
/// This is the shape that made the page render every requirement once for each requirement in
/// its file. `openspec show --json` answers a change touching two capabilities with **ten** rows —
/// six for one and four for the other, one per requirement, each carrying that single requirement
/// in both a `requirement` and a `requirements` field. Reading the delta file once per row and
/// returning everything in it therefore squared: six rows × six requirements, and a reader saw
/// the same paragraph fourteen times with nothing on screen suggesting why.
///
/// So the rows are grouped first and each file is read **once**. What the rows are actually good
/// for is telling cide *which* `(capability, operation)` pairs a change touches; the contents come
/// from the file, for the reason [`requirements_in`]'s caller states — the JSON carries neither
/// requirement names nor scenario titles.
///
/// First-seen order is kept, so the page lists capabilities in the order the change does rather
/// than in whatever order a map would have produced.
fn group_deltas(
    rows: Vec<model::DeltaRow>,
    artifacts: &[cide_ipc::SpecArtifact],
) -> Vec<cide_ipc::SpecDelta> {
    let mut groups: Vec<(cide_ipc::SpecId, cide_ipc::DeltaOperation, String, usize)> = Vec::new();
    for row in rows {
        let spec = cide_ipc::SpecId(row.spec.clone());
        let operation = model::operation(&row.operation);
        let carried = row.requirement_count();
        match groups
            .iter_mut()
            .find(|(id, op, _, _)| *id == spec && *op == operation)
        {
            Some((_, _, _, count)) => *count += carried,
            None => groups.push((spec, operation, row.description, carried)),
        }
    }

    groups
        .into_iter()
        .map(|(spec, operation, description, reported)| {
            let requirements = delta_file(artifacts, &spec)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .map(|text| requirements_in(&text, operation))
                .unwrap_or_default();
            if requirements.len() != reported {
                // Not a failure — `validate` will have something to say about a delta file the
                // scanner and the CLI read differently — but it is the one disagreement that
                // would make an edit address the wrong block, so it is logged rather than
                // smoothed over.
                tracing::debug!(
                    spec = %spec,
                    scanned = requirements.len(),
                    reported,
                    "the delta file and openspec disagree about how many requirements it holds"
                );
            }
            cide_ipc::SpecDelta {
                spec,
                operation,
                // The first row's, and it is a *per-requirement* sentence upstream
                // ("Add requirement: …"), so it describes the group only loosely. Kept because
                // the DTO has the field and dropping it would lose the only prose the CLI
                // offers about a delta; nothing renders it today.
                description,
                requirements,
                // A rename is a property of one requirement, not of a capability's whole delta,
                // so it does not survive grouping. `RENAMED` sections are read from the file
                // like everything else.
                rename: None,
            }
        })
        .collect()
}

/// The directory holding the archived form of `change`, if the project has one.
///
/// `openspec archive` moves `openspec/changes/<name>/` to
/// `openspec/changes/archive/<YYYY-MM-DD>-<name>/`, so the folder name is not the change name and
/// a lookup by name finds nothing.
///
/// The stamp is stripped **as a date**, not as "anything before the last match": a change called
/// `auth` would otherwise be answered by an archived `2026-01-01-fix-auth`, since that ends with
/// `-auth`. Newest wins when a name has been archived more than once, which a lexicographic max
/// gives for free on an ISO stamp.
pub fn archived_dir(root: &Path, change: &ChangeName) -> Option<PathBuf> {
    let archive = spec_path(root).join("changes").join(ARCHIVE_DIR);
    let mut best: Option<(String, PathBuf)> = None;
    for entry in std::fs::read_dir(&archive).ok()?.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if undated(&name) != change.as_str() {
            continue;
        }
        if best.as_ref().is_none_or(|(seen, _)| name > *seen) {
            best = Some((name, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

/// `2026-08-27-add-dark-mode` → `add-dark-mode`; anything else unchanged.
fn undated(name: &str) -> &str {
    let bytes = name.as_bytes();
    let dated = bytes.len() > 11
        && bytes[..10].iter().enumerate().all(|(at, byte)| {
            if at == 4 || at == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && bytes[10] == b'-';
    if dated { &name[11..] } else { name }
}

/// An archived change, read off the disk because no CLI command will.
///
/// # What this reads, and what it refuses to invent
///
/// The **deltas** — through [`block`], the same byte-range scanner the write path uses and the
/// one exception ADR 0012 already sanctions — and the **files that are there**, which is a
/// directory listing rather than a parse. Both are recoverable exactly.
///
/// The checklist and the verdict are **not**. Counting ticked boxes means knowing which file is
/// the checklist and what counts as an item, and both are the schema's business; validating means
/// running a command that cannot see this directory. So `progress` is empty and `validation` is
/// vacuously clean, and [`cide_ipc::SpecOrigin::Archived`] is the flag that tells every surface
/// not to draw either. An archived change is a *record*, not a workspace.
///
/// The artifact ids are file **stems**, for the same reason: there is no schema to ask. That
/// happens to give `proposal`, `design` and `tasks` on a default-schema project, which is what the
/// card's disclosures look for, and a custom schema's own names on one that has them.
pub fn archived_change(root: &Path, change: &ChangeName) -> Option<cide_ipc::SpecChange> {
    let dir = archived_dir(root, change)?;
    let folder = dir.file_name()?.to_str()?.to_string();

    let mut artifacts: Vec<cide_ipc::SpecArtifact> = Vec::new();
    let mut documents: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            (path.extension().and_then(|ext| ext.to_str()) == Some("md")).then_some((stem, path))
        })
        .collect();
    documents.sort();
    for (stem, path) in documents {
        artifacts.push(cide_ipc::SpecArtifact {
            generates: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
            id: stem,
            // Everything in an archive is finished by construction — the change was accepted.
            state: cide_ipc::ArtifactState::Done,
            existing: vec![path],
        });
    }

    let mut delta_paths: Vec<(cide_ipc::SpecId, PathBuf)> = Vec::new();
    collect_deltas(&dir.join("specs"), &mut Vec::new(), &mut delta_paths);
    delta_paths.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    if !delta_paths.is_empty() {
        artifacts.push(cide_ipc::SpecArtifact {
            id: "specs".to_string(),
            generates: "specs/*/spec.md".to_string(),
            state: cide_ipc::ArtifactState::Done,
            existing: delta_paths.iter().map(|(_, path)| path.clone()).collect(),
        });
    }

    let mut deltas: Vec<cide_ipc::SpecDelta> = Vec::new();
    for (spec, path) in delta_paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Every operation, because the file says which it uses and cide has no CLI answer to ask
        // — a delta file may carry more than one section, and an empty one is simply not there.
        for operation in cide_ipc::DeltaOperation::EVERY {
            let requirements = requirements_in(&text, operation);
            if requirements.is_empty() {
                continue;
            }
            deltas.push(cide_ipc::SpecDelta {
                spec: spec.clone(),
                operation,
                // The CLI's per-requirement sentence, which nothing renders and which no
                // directory holds. Empty rather than invented.
                description: String::new(),
                requirements,
                // `FROM:`/`TO:` is inside the block the scanner already carries verbatim; the
                // CLI's parsed form of it is not recoverable from a directory, and inventing one
                // is exactly what this function does not do.
                rename: None,
            });
        }
    }

    Some(cide_ipc::SpecChange {
        name: change.clone(),
        title: change.as_str().to_string(),
        deltas,
        artifacts,
        progress: cide_ipc::SpecProgress {
            tasks: Vec::new(),
            total: 0,
            completed: 0,
        },
        validation: cide_ipc::SpecValidation {
            valid: true,
            issues: Vec::new(),
        },
        origin: cide_ipc::SpecOrigin::Archived { folder },
    })
}

/// Every `spec.md` under an archived change's `specs/`, with the capability id its path spells.
///
/// Recursive because a capability id may itself be a path (`identity/user-auth`), which is the
/// same fact `delta_file` matches on components for.
fn collect_deltas(
    dir: &Path,
    segments: &mut Vec<String>,
    found: &mut Vec<(cide_ipc::SpecId, PathBuf)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            segments.push(name);
            collect_deltas(&path, segments, found);
            segments.pop();
        } else if name == "spec.md" && !segments.is_empty() {
            found.push((cide_ipc::SpecId(segments.join("/")), path));
        }
    }
}

/// The file holding one capability's delta, from the paths the CLI reported.
///
/// Matched on the path's tail **components** and never on a substring: a spec id can itself be a
/// path (`identity/user-auth`), and a substring match would let one capability's file answer for
/// another whose id is a suffix of it.
fn delta_file<'a>(
    artifacts: &'a [cide_ipc::SpecArtifact],
    spec: &cide_ipc::SpecId,
) -> Option<&'a Path> {
    let wanted: Vec<String> = std::iter::once("specs".to_string())
        .chain(spec.as_str().split('/').map(str::to_string))
        .chain(std::iter::once("spec.md".to_string()))
        .collect();
    artifacts
        .iter()
        .flat_map(|artifact| artifact.existing.iter())
        .find(|path| {
            let have: Vec<String> = path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect();
            have.len() >= wanted.len() && have[have.len() - wanted.len()..] == *wanted
        })
        .map(PathBuf::as_path)
}

/// Every requirement under one operation's section of a delta file.
fn requirements_in(
    text: &str,
    operation: cide_ipc::DeltaOperation,
) -> Vec<cide_ipc::SpecRequirement> {
    block::names(text, operation.header())
        .into_iter()
        .filter_map(|name| {
            let found = block::find(text, operation.header(), &name).ok()?;
            let raw = text.get(found.start..found.end)?;
            let parsed = block::parse(raw);
            Some(cide_ipc::SpecRequirement {
                name: parsed.name,
                text: parsed.text,
                scenarios: parsed
                    .scenarios
                    .into_iter()
                    .map(|scenario| cide_ipc::SpecScenario {
                        title: scenario.title,
                        body: scenario.body,
                    })
                    .collect(),
                // Trailing whitespace trimmed, matching what the writer splices back — so a
                // block read and written unchanged really is unchanged.
                block: raw.trim_end().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cide-spec-archive-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directories");
        std::fs::write(path, text).expect("the file");
    }

    /// `2026-08-27-add-dark-mode` is `add-dark-mode`, and `2026-01-01-fix-auth` is **not** `auth`.
    ///
    /// The second half is the whole reason the stamp is stripped as a *date* rather than by
    /// "anything up to the last match": `ends_with("-auth")` is true of `fix-auth`, so a change
    /// called `auth` would have been answered by somebody else's archived directory — with real
    /// requirements in it, on the card of a task that had nothing to do with it.
    #[test]
    fn the_archive_stamp_is_stripped_as_a_date() {
        assert_eq!(undated("2026-08-27-add-dark-mode"), "add-dark-mode");
        assert_eq!(undated("add-dark-mode"), "add-dark-mode");
        assert_eq!(undated("2026-01-01-fix-auth"), "fix-auth");
        assert_eq!(
            undated("v1-add-dark-mode"),
            "v1-add-dark-mode",
            "not a date"
        );
        assert_eq!(
            undated("2026-08-27"),
            "2026-08-27",
            "a stamp and nothing else"
        );
    }

    /// An archived change reads back: which directory, its documents, and its requirements.
    ///
    /// No CLI command can answer any of this — `show` resolves `changes/<name>/proposal.md` and
    /// an archived change is not there — so a task linked to one rendered as a failure from the
    /// moment its work was accepted.
    #[test]
    fn an_archived_change_reads_back_off_the_disk() {
        let root = scratch("read");
        let dir = "openspec/changes/archive/2026-08-27-add-dark-mode";
        write(&root, &format!("{dir}/proposal.md"), "## Why\n\nBecause.\n");
        write(&root, &format!("{dir}/design.md"), "# Design\n");
        write(&root, &format!("{dir}/tasks.md"), "- [x] One\n- [x] Two\n");
        write(
            &root,
            &format!("{dir}/specs/dark-mode/spec.md"),
            "## ADDED Requirements\n\n### Requirement: Theme switching\n\nThe app SHALL switch              themes.\n\n#### Scenario: Picks dark\n\n- **WHEN** a\n- **THEN** b\n",
        );
        // A capability whose id is itself a path — the case `delta_file` matches on components
        // for, and the reason the walk is recursive.
        write(
            &root,
            &format!("{dir}/specs/identity/user-auth/spec.md"),
            "## MODIFIED Requirements\n\n### Requirement: Sign in\n\nThe app SHALL sign in.\n\n             #### Scenario: Works\n\n- **WHEN** a\n- **THEN** b\n",
        );
        // The decoy from the test above, on disk this time.
        write(
            &root,
            "openspec/changes/archive/2026-01-01-fix-auth/proposal.md",
            "x",
        );

        let change = ChangeName("add-dark-mode".into());
        let read = archived_change(&root, &change).expect("the archived change");

        match &read.origin {
            cide_ipc::SpecOrigin::Archived { folder } => {
                assert_eq!(
                    folder, "2026-08-27-add-dark-mode",
                    "the stamp is the record"
                )
            }
            other => panic!("read as {other:?}, not as archived"),
        }

        // The deltas, recovered by the same scanner the write path uses.
        let mut got: Vec<(String, cide_ipc::DeltaOperation, usize)> = read
            .deltas
            .iter()
            .map(|delta| {
                (
                    delta.spec.as_str().to_string(),
                    delta.operation,
                    delta.requirements.len(),
                )
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                ("dark-mode".to_string(), cide_ipc::DeltaOperation::Added, 1),
                (
                    "identity/user-auth".to_string(),
                    cide_ipc::DeltaOperation::Modified,
                    1
                ),
            ]
        );
        assert_eq!(read.deltas[0].requirements[0].name, "Theme switching");
        assert!(
            read.deltas[0].requirements[0]
                .block
                .contains("SHALL switch")
        );

        // The documents, by stem — which is what the card's disclosures look up.
        let mut ids: Vec<&str> = read.artifacts.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["design", "proposal", "specs", "tasks"]);
        let tasks = read
            .artifacts
            .iter()
            .find(|a| a.id == "tasks")
            .expect("the checklist file is listed");
        assert!(tasks.existing[0].ends_with("tasks.md"));

        // And the two fields a directory cannot answer are **empty**, not guessed. A `0/0`
        // progress bar over finished work, or a green verdict nobody validated, is a worse claim
        // than the refusal this replaced — `SpecOrigin::Archived` is what tells a surface so.
        assert_eq!(read.progress.total, 0);
        assert!(read.progress.tasks.is_empty());
        assert!(read.validation.issues.is_empty());

        assert!(
            archived_change(&root, &ChangeName("auth".into())).is_none(),
            "`2026-01-01-fix-auth` is not the archived form of `auth`"
        );
        assert!(archived_change(&root, &ChangeName("nothing".into())).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Archived twice under the same name: the newest wins.
    #[test]
    fn the_newest_archive_of_a_name_is_the_one_that_answers() {
        let root = scratch("newest");
        for stamp in ["2026-01-05", "2026-09-09", "2026-03-01"] {
            write(
                &root,
                &format!("openspec/changes/archive/{stamp}-add-dark-mode/proposal.md"),
                "x",
            );
        }
        let dir = archived_dir(&root, &ChangeName("add-dark-mode".into())).expect("a directory");
        assert!(
            dir.ends_with("2026-09-09-add-dark-mode"),
            "{}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_short_proposal_why_is_extended_rather_than_written_too_short() {
        // Upstream's floor is 50 characters and its refusal names a length, which from a panel
        // reads as cide having produced something malformed.
        let why = proposal_why("Add dark mode", "");
        assert!(why.chars().count() >= 50, "{why}");
        assert!(why.contains("Add dark mode"));

        let long = "The editor is unreadable at night and everyone has asked for this twice.";
        assert_eq!(proposal_why("t", long), long, "a real body is left alone");

        // A short body is kept *and* extended — the user's words are never thrown away.
        let why = proposal_why("Add dark mode", "It is too bright.");
        assert!(why.starts_with("It is too bright."), "{why}");
        assert!(why.chars().count() >= 50, "{why}");
    }
}
