//! The project-wide symbol walk.
//!
//! A structural twin of `cide_search::content::Search`: the same `ignore::WalkBuilder` settings,
//! the same `ParallelVisitorBuilder`, the same "visitors send batches, the receiving thread
//! enforces the total cap" division. Reading them side by side should be easy, because a walk
//! that descends where the file tree does not produces rows the user cannot open.
//!
//! Four things differ, and each is the reason this is not just a call into that one:
//!
//! 1. **The extension gate comes first.** Only `.rs` and `.go` are ever opened. On a repository
//!    where source is a minority of the files — which is every repository with a `node_modules`,
//!    a `target`, or a directory of fixtures — this is the single largest cost control, and it
//!    happens before any I/O.
//! 2. **One [`Parser`] per visitor.** A `Parser` owns the compiled grammar; building one per file
//!    would be most of what the walk spends its time on. `Parser` is `Send` and not `Sync`, which
//!    is exactly the `Visitor::buf` shape `content.rs` already uses.
//! 3. **`str::from_utf8` instead of a NUL probe.** A `.rs` that is not UTF-8 is skipped, matching
//!    `cide-fs`'s posture — and it costs nothing, since the bytes are about to be scanned anyway.
//! 4. **The whole-index cap is symbols, not files**, because a file's cost here is its
//!    declarations rather than its length.
//!
//! # What a skipped file does *not* do
//!
//! It does not raise, and it does not report. A file too large, not UTF-8, or over its parse
//! budget contributes nothing to a walk and nothing to the log — the same rule `content.rs`
//! states for the same reason: a walk of a repository that reported every unreadable file would
//! bury the answer. The **single-file** path ([`crate::outline`]) reports all three, because
//! there the user asked about that exact file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use cide_ipc::Symbol;
use tree_sitter::Parser;

use crate::Lang;
use crate::extract::{Limits, parse_with, symbols_of};

/// Should this path be walked? `is_dir` is passed so a caller can prune a whole subtree.
///
/// Injected rather than computed here, exactly as `cide_search::content::Admits` is: the ignore
/// rules live in `cide-fs`, this crate must not depend on it, and two implementations of "is this
/// file ignored" is how a search starts disagreeing with the file tree.
pub type Admits<'a> = &'a (dyn Fn(&Path, bool) -> bool + Sync);

/// One root of a project, and what to call files inside it.
#[derive(Debug, Clone)]
pub struct WalkRoot {
    pub path: PathBuf,
    /// Prefixed onto `rel` in a multi-root project, so one file is named the same here, in the
    /// file picker and in the search panel.
    pub label: Option<String>,
}

/// One file's declarations, as they leave the walk.
#[derive(Debug, Clone)]
pub struct WalkedFile {
    /// Absolute.
    pub path: String,
    /// Root-relative, label-prefixed in a multi-root project.
    pub rel: String,
    pub lang: Lang,
    pub symbols: Vec<Symbol>,
}

/// What the walk did.
#[derive(Debug, Clone, Copy, Default)]
pub struct WalkOutcome {
    /// Source files parsed — not files walked, since a `.md` is neither.
    pub files: u32,
    pub symbols: u32,
    /// The whole-index cap stopped it. Counts are floors.
    pub truncated: bool,
}

/// Walk every root, parsing what has a grammar, streaming a batch per file to `sink`.
///
/// `sink` runs on the calling thread, one file at a time, so it needs no lock of its own — the
/// same arrangement `content::Search::run` uses, and for the same reason: it is the only place
/// that can enforce an exact total.
pub fn walk_symbols(
    roots: &[WalkRoot],
    limits: Limits,
    admits: Option<Admits<'_>>,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(WalkedFile),
) -> WalkOutcome {
    let mut outcome = WalkOutcome::default();
    let multi = roots.len() > 1;
    for root in roots {
        if cancel.load(Ordering::Acquire) || outcome.truncated {
            break;
        }
        walk_root(root, multi, limits, admits, cancel, sink, &mut outcome);
    }
    outcome
}

fn walk_root(
    root: &WalkRoot,
    multi: bool,
    limits: Limits,
    admits: Option<Admits<'_>>,
    cancel: &AtomicBool,
    sink: &mut dyn FnMut(WalkedFile),
    outcome: &mut WalkOutcome,
) {
    let mut builder = ignore::WalkBuilder::new(&root.path);
    builder
        // Every one of these mirrors `cide_fs::index::walk_root` and
        // `cide_search::content::Search::run`. They are the defaults apart from `parents` and
        // `require_git`, and they are spelled out for the same reason they are spelled out
        // there: the three walks have to be readable side by side.
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(false)
        .require_git(false)
        .follow_links(false)
        .threads(limits.threads);

    let (tx, rx) = mpsc::channel::<WalkedFile>();

    std::thread::scope(|scope| {
        let walker = builder.build_parallel();
        let mut factory = Visitors {
            tx,
            root,
            multi,
            limits,
            admits,
            cancel,
        };
        scope.spawn(move || walker.visit(&mut factory));

        // The receiver is the only place the whole-index cap is enforced. A visitor applies
        // `max_symbols_per_file` and consults `cancel`, and that is all — a shared running total
        // would be an atomic per file on N threads that could still overshoot between the load
        // and the send, and it would leave the trim here anyway.
        for file in rx {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let room = limits.max_symbols.saturating_sub(outcome.symbols as usize);
            if file.symbols.len() > room {
                outcome.truncated = true;
                // Stops the walkers. Without it this loop would sit here while a walk of the
                // whole repository produced symbols that are already over the cap.
                cancel.store(true, Ordering::Release);
                break;
            }
            outcome.files += 1;
            outcome.symbols += file.symbols.len() as u32;
            sink(file);
        }
    });
}

/// `ignore` asks this for one visitor per walker thread.
struct Visitors<'a> {
    tx: mpsc::Sender<WalkedFile>,
    root: &'a WalkRoot,
    multi: bool,
    limits: Limits,
    admits: Option<Admits<'a>>,
    cancel: &'a AtomicBool,
}

impl<'a, 's> ignore::ParallelVisitorBuilder<'s> for Visitors<'a>
where
    'a: 's,
{
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        Box::new(Visitor {
            tx: self.tx.clone(),
            root: self.root,
            multi: self.multi,
            limits: self.limits,
            admits: self.admits,
            cancel: self.cancel,
            // One per thread, reused across every file that thread visits. This is the whole
            // reason `symbols_of` and `parse_with` are separate from `outline_symbols`.
            parser: Parser::new(),
            buf: String::new(),
        })
    }
}

struct Visitor<'a> {
    tx: mpsc::Sender<WalkedFile>,
    root: &'a WalkRoot,
    multi: bool,
    limits: Limits,
    admits: Option<Admits<'a>>,
    cancel: &'a AtomicBool,
    parser: Parser,
    /// Reused across files, so a walk of 20,000 files does not allocate 20,000 buffers.
    buf: String,
}

impl Visitor<'_> {
    /// Parse one file, or decide not to. `None` when nothing was opened.
    fn scan(&mut self, entry: &ignore::DirEntry) -> Option<()> {
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            return None;
        }

        // **First**, before any I/O. See the module docs.
        let lang = Lang::of_path(path)?;

        // Non-UTF-8 paths are dropped rather than lossily converted, as they are in the file
        // index: the path is the identifier the frontend hands back to open a row, and
        // `to_string_lossy` produces one that names no file.
        let path_str = path.to_str()?;
        let rel = path.strip_prefix(&self.root.path).ok()?.to_str()?;

        if let Some(admits) = self.admits
            && !admits(path, false)
        {
            return None;
        }

        let meta = entry.metadata().ok()?;
        if meta.len() > self.limits.max_file_bytes {
            return None;
        }

        self.buf.clear();
        // `read_to_string` rather than reading bytes and sniffing: a `.rs` that is not UTF-8 is
        // skipped, and this is the same call that would have to decode it anyway.
        {
            use std::io::Read;
            let mut file = std::fs::File::open(path).ok()?;
            file.read_to_string(&mut self.buf).ok()?;
        }

        let tree = parse_with(&mut self.parser, lang, &self.buf, self.limits)?;
        let (symbols, _) = symbols_of(&tree, &self.buf, lang, self.limits);
        if symbols.is_empty() {
            return None;
        }

        let rel = match &self.root.label {
            Some(label) if self.multi => format!("{label}/{rel}"),
            _ => rel.to_string(),
        };
        // A closed receiver means the walk was cancelled; the send failing is the signal, not an
        // error worth reporting.
        self.tx
            .send(WalkedFile {
                path: path_str.to_string(),
                rel,
                lang,
                symbols,
            })
            .ok()
    }
}

impl ignore::ParallelVisitor for Visitor<'_> {
    fn visit(&mut self, entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        if self.cancel.load(Ordering::Acquire) {
            return ignore::WalkState::Quit;
        }
        let Ok(entry) = entry else {
            // An unreadable directory is one the user cannot open from the tree either. Skipped
            // rather than reported, per the module docs.
            return ignore::WalkState::Continue;
        };
        if let Some(admits) = self.admits
            && entry.file_type().is_some_and(|t| t.is_dir())
            && !admits(entry.path(), true)
        {
            return ignore::WalkState::Skip;
        }
        self.scan(&entry);
        ignore::WalkState::Continue
    }
}
