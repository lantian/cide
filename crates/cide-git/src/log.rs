//! One page of commit log, walked by hand. (M19)
//!
//! # Why `git2::Revwalk` is not used, at all
//!
//! This is the least obvious decision in the crate and the one a later reader is most likely to
//! "simplify" away, so it is written down first, with the source it was verified against.
//!
//! The point of [`LogQuery::scan_limit`] is that a filtered walk over a long history is
//! *bounded*: the page comes back short, honest and resumable instead of walking forty thousand
//! commits on an IPC thread to fill a page of two. A budget can only bound a loop that is ours.
//! libgit2's is not.
//!
//! In `libgit2 1.9.6`, as vendored by `libgit2-sys 0.18.7+1.9.6`:
//!
//! * `revwalk.c:762-763` — `if (walk->sorting != GIT_SORT_NONE) walk->limited = 1;`
//! * `revwalk.c:660` — `if (walk->limited && (error = limit_list(&commits, walk, commits)) < 0)`
//! * `revwalk.c:471-505` — `limit_list` is a `while (list)` loop that pops a commit, calls
//!   `add_parents_to_list`, and keeps going until the list is empty. It drains the **entire
//!   reachable set**, and it runs inside `prepare_walk`, which runs inside the *first*
//!   `git_revwalk_next`.
//!
//! So `Sort::TIME` does not stream either: the first `next()` on a hundred-thousand-commit
//! repository walks all hundred thousand before it yields row one, and a scan budget wrapped
//! around it would bound nothing at all. The only mode that streams is `Sort::NONE`
//! (`revwalk.c:522`, `if (!walk->limited)`, which adds parents lazily as commits are popped),
//! and libgit2 documents its order as arbitrary — useless for a log.
//!
//! Performance is **not** the reason, and saying so matters: this repository is a few hundred
//! commits and `limit_list` costs nothing on it. The features are the reason, and four of them
//! are unbuildable on a walk we do not own:
//!
//! | feature | why libgit2's walk cannot serve it |
//! | --- | --- |
//! | graph lanes stable across pages | the continuation needs a *serialisable* frontier; libgit2's pqueue is opaque |
//! | rename-follow through merges | the tracked path must ride on a frontier **entry**, not on the walk |
//! | merged multi-root log | one heap with entries tagged by repo — merged *is* the single case with k>1 |
//! | an honest `scanned` count | it is our loop counter, or it is a guess |
//!
//! The mitigation is the house pattern for the other dangerous code in this crate: **the real
//! `git` binary is the oracle.** `tests/log.rs` asserts the emitted oid list equal to
//! `git log --format=%H` under each mode, exactly as `tests/patch_props.rs` asserts staging
//! against `git apply --cached`.
//!
//! # The ordering key
//!
//! A parent is queued with `key = min(committer_time(parent), key(child))`, never with its own
//! raw committer time. See [`Pending::key`] — that clamp is what makes paging exact, and it is
//! the second thing not to "simplify".
//!
//! # What suppresses a row, and what suppresses an edge
//!
//! Two invariants that look alike and are not:
//!
//! * **The author and text filters suppress a row and never the traversal.** A commit whose
//!   author does not match is still walked *through*, because its ancestors may match. Pruning
//!   the traversal on a text filter makes every ancestor of a non-matching commit vanish, which
//!   is a log that silently omits most of its own answers.
//! * **Simplification suppresses a row *and* rewrites the edge.** A commit dropped by
//!   [`Simplify::Default`] must not leave the graph pointing at a row that is not in the list,
//!   so the child's edge is re-pointed at the followed parent and the hops are counted into
//!   [`CommitRow::pruned`].
//!
//! # No cached state
//!
//! Every [`Repository`] this module opens is opened and dropped inside one call, per the crate's
//! rule (`lib.rs`, *No cached state*) — the merged scope included. The continuation that has to
//! survive is a **value**, [`LogResume`], which the caller holds and hands back and which is
//! validated on arrival rather than trusted.
//!
//! # Cancellation is a borrowed flag
//!
//! [`log_cancellable`] takes an `&AtomicBool` and polls it once per commit; [`log`] is the same
//! walk over a flag nobody can reach. The flag is **borrowed** and never owned, because of the
//! rule one paragraph up: this crate holds no state, so the thing that knows a page has been
//! superseded — a registry keyed by tool tab, `cide_app::cmd::log::LogRegistry` — lives in the app
//! crate and lends a reference in for the duration of one call.
//!
//! `&AtomicBool` and **not** `&dyn Fn() -> bool`. A closure has to be `Send + Sync` to be set from
//! a Tauri worker while the walk polls it, so those two bounds would have to be threaded through
//! every signature that carries it; it buys nothing an atomic load does not; and it would be a
//! *second* shape for cancellation in a workspace that already has one. `cide_search::content`'s
//! `Search::cancel` is that one, and this is deliberately the same seam, down to the
//! `Ordering::Acquire` on the poll — a reader of either should not have to learn two conventions.
//!
//! **A cancelled page is a partial answer, not a failure.** The rows found so far come back with
//! [`CommitPage::cancelled`] set, and `Ok`. An error would throw away rows that are correct, and
//! would reach the panel as a red sentence for the ordinary act of typing one more character into
//! the filter box — see that field's own note.

use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use cide_ipc::git::{GitError, RepoInfo};
use cide_ipc::history::{
    CommitPage, CommitRow, FrontierRef, GraphOff, LaneResume, LogCursor, LogGraph, LogQuery,
    LogRefs, LogResume, LogScope, LogStop, RefChip, RefKind, RenameHop, RepoPage, RepoResume,
    Simplify,
};
use git2::{Oid, Repository, RevparseMode};
use indexmap::IndexMap;

use crate::lanes::Lanes;
use crate::{Result, Wrap, repo as repo_mod};

// --- budgets ---------------------------------------------------------------------------------

/// Rows a page produces when the caller does not say.
pub const LIMIT_DEFAULT: u32 = 100;
/// The most rows one page may produce, whatever the caller asks for. A frontend bug that asks
/// for a million rows must not be able to allocate them here.
pub const LIMIT_MAX: u32 = 1_000;
/// Commits one walk may examine when the caller does not say.
pub const SCAN_DEFAULT: u32 = 20_000;
/// The most commits one walk may examine. Half a million commits of tree comparison is already
/// several seconds; past that, the answer the user wants is a different query.
pub const SCAN_MAX: u32 = 500_000;
/// How far [`LogCursor::At`] and [`LogCursor::After`] will walk looking for their anchor.
///
/// Its own budget, and deliberately far larger than [`SCAN_MAX`]: seeking to a commit the user
/// clicked is *positioning*, not searching, and a "reveal this commit" that gave up after twenty
/// thousand rows would fail on exactly the old commit someone is trying to find. The commits it
/// walks are counted into the reported `scanned` — the number stays honest — but they are **not**
/// charged against `scan_limit`, which is the budget for the filter.
pub const MAX_SEEK: u32 = 1_000_000;
/// How many recently-visited oids ride in the resume token as the clock-skew guard.
pub const RECENT_CAP: usize = 64;
/// How many refs [`ref_chips`] will peel. A repository with more refs than this is a mirror of
/// something, and the chips are decoration — they must not turn one page of log into a hundred
/// thousand `peel` calls.
pub const REF_CHIP_CAP: usize = 5_000;
/// How many names one lane of a `--follow` walk may track at once.
///
/// Above one only because a rename can be ambiguous mid-walk and both candidates have to stay
/// live until one dies out. Unbounded, it becomes a way for a repository that renames its whole
/// tree to make every frontier entry carry every path in it.
pub const MAX_TRACKED_PATHS: usize = 8;
/// What one rename-detection pass costs against `scan_limit`.
///
/// A hop is a whole-tree diff plus `find_similar`; charging it as one commit would let a follow
/// walk burn its entire budget without the number moving.
pub const RENAME_SCAN_COST: u32 = 64;
/// Above this many deltas, rename detection is skipped for that commit and `follow_capped` is
/// reported. `find_similar` is quadratic in the unmatched adds and deletes, and a tree-wide move
/// is exactly the commit that produces tens of thousands of them.
pub const RENAME_MAX_DELTAS: usize = 2_000;
/// How many rename hops **one commit** may contribute.
///
/// Walk-wide hops are deliberately *uncapped* — a hop fires once per real rename, and a cap
/// there is the bug the first design had: it stopped following at an arbitrary depth and then
/// reported the truncated history as though it were the whole one. What is capped is the *cost*.
/// A tracked set of at most [`MAX_TRACKED_PATHS`] names can have every one of them renamed by a
/// single tree-wide move, and this bounds what one such commit may add to the frontier and to
/// [`CommitPage::renames`].
pub const MAX_FOLLOW_HOPS: usize = 10;
/// libgit2's own `SLOP` (`revwalk.c:443`): how many uninteresting commits to keep looking at
/// after the interesting ones run out. Ported, not re-derived — see [`still_interesting`].
pub const SLOP: i32 = 5;

/// Format version of a [`LogResume`] token minted here.
///
/// Bumped whenever the *meaning* of a field changes, not merely its contents. An older build
/// meeting a newer token must recognise it and refuse, which is why [`LogResume`] is one of the
/// few wire structs without `deny_unknown_fields`.
const TOKEN_VERSION: u32 = 1;

/// Ceiling on the oid set carried in [`RepoResume::hidden`].
///
/// The set is already pruned to oids that could be re-reached from the frontier, which under a
/// sane clock is the tie group at the page boundary — a handful. Under pathological skew it can
/// grow, and the token crosses the IPC wire on every scroll, so it gets a hard ceiling as well
/// as a rule. Losing an entry costs at worst one duplicated row, which the row key makes
/// cosmetic; an unbounded token costs every page.
const HIDDEN_CAP: usize = RECENT_CAP * 16;

// --- ref chips ---------------------------------------------------------------------------------

/// Every ref that peels to a commit, keyed by that commit.
///
/// One pass with two uses, which is why it is public and why it is not folded into the walk: the
/// chips the rows carry, and the tips [`LogRefs::All`] pushes. Doing it twice would be two
/// answers to "what is a ref here", and the exclusions below are exactly where they would
/// differ.
pub fn ref_chips(repo: &Repository) -> Result<HashMap<Oid, Vec<RefChip>>> {
    let mut out: HashMap<Oid, Vec<RefChip>> = HashMap::new();

    // Where HEAD points, and whether it is attached. Both are needed: an attached HEAD marks its
    // *branch* chip current, a detached one earns a chip of its own.
    let head = repo.head().ok();
    let head_oid = head
        .as_ref()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id());
    let head_ref = head
        .as_ref()
        .filter(|h| h.is_branch())
        .and_then(|h| h.name().ok().map(str::to_owned));
    let detached = repo.head_detached().unwrap_or(false);

    let mut seen = 0usize;
    for reference in repo.references().wrap()? {
        let Ok(reference) = reference else { continue };
        seen += 1;
        if seen > REF_CHIP_CAP {
            tracing::warn!(
                cap = REF_CHIP_CAP,
                "too many refs to chip; the rest are dropped"
            );
            break;
        }
        // Non-UTF-8 refnames are skipped rather than rendered lossily, for the reason
        // `branch::collect` gives: a chip whose name the user never saw is a chip whose context
        // menu acts on something else.
        let Ok(full) = reference.name().map(str::to_owned) else {
            continue;
        };
        let (kind, name) = if let Some(rest) = full.strip_prefix("refs/heads/") {
            (RefKind::LocalBranch, rest.to_string())
        } else if let Some(rest) = full.strip_prefix("refs/remotes/") {
            // `origin/HEAD` is a symbolic ref pointing at another row in this same list —
            // exactly the exclusion `branch::collect` makes, and for the same reason: it is a
            // second name for a branch already chipped, and drawing both puts two chips on one
            // commit saying the same thing.
            if rest.ends_with("/HEAD") {
                continue;
            }
            (RefKind::RemoteBranch, rest.to_string())
        } else if let Some(rest) = full.strip_prefix("refs/tags/") {
            (RefKind::Tag, rest.to_string())
        } else {
            continue;
        };
        // `peel_to_commit` and not `target()`: an **annotated** tag's ref points at a tag object,
        // not at a commit, so `target()` would key the chip by the tag's own oid and the chip
        // would never appear on any row.
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        let current = head_ref.as_deref() == Some(full.as_str());
        out.entry(commit.id()).or_default().push(RefChip {
            kind,
            name,
            full,
            current,
        });
    }

    // A detached HEAD gets its own chip; an attached one does not, because the branch chip
    // beside it already says where HEAD is.
    if detached && let Some(oid) = head_oid {
        out.entry(oid).or_default().push(RefChip {
            kind: RefKind::Head,
            name: "HEAD".to_string(),
            full: "HEAD".to_string(),
            current: true,
        });
    }

    // A total order inside one commit's chips, so two refreshes cannot reshuffle them: HEAD
    // first, then local branches, remote branches and tags, each alphabetically.
    for chips in out.values_mut() {
        chips.sort_by(|a, b| {
            kind_rank(a.kind)
                .cmp(&kind_rank(b.kind))
                .then_with(|| a.name.cmp(&b.name))
        });
    }
    Ok(out)
}

fn kind_rank(kind: RefKind) -> u8 {
    match kind {
        RefKind::Head => 0,
        RefKind::LocalBranch => 1,
        RefKind::RemoteBranch => 2,
        RefKind::Tag => 3,
    }
}

// --- the frontier ------------------------------------------------------------------------------

/// One commit waiting to be walked.
#[derive(Debug, Clone)]
struct Pending {
    /// The sort key, which is **not** the commit's own time.
    ///
    /// A parent is queued with `min(its own committer time, the key of the child that reached
    /// it)`. Because every insertion happens on a *pop of the maximum*, and is therefore `<=`
    /// the key just popped, the popped key sequence is non-increasing **by construction** —
    /// which is what makes keyset paging across a boundary provably exact rather than nearly
    /// right. Raw committer time does not have that property: one commit with a bad clock (an
    /// imported history, a CI box in the wrong year, a rebase run on a laptop with a dead
    /// battery) puts a parent *ahead* of rows already emitted, and the next page repeats them.
    ///
    /// It is also what makes a skewed history terminate: a clamped key can only fall, so the
    /// walk cannot ping-pong between two commits whose stamps disagree.
    key: i64,
    /// The path filter as it stands **at this commit**, after every rename followed so far.
    ///
    /// On the frontier entry and not on the walk. That is the fix for the bug the first design
    /// had: a commit reachable from two children — every merge — had its tracked name
    /// *overwritten* by whichever child was popped last, so branch A's rename leaked into branch
    /// B's lane and B's history stopped at a rename it never made. Here the entry is keyed by
    /// oid and the paths are **unioned**.
    ///
    /// Empty when there is no path filter.
    paths: Vec<Rc<str>>,
    /// False for a commit hidden by a range revspec (`a..b`'s left side) and for its ancestors.
    /// An uninteresting commit is walked — its parents have to be marked too — and never
    /// emitted.
    interesting: bool,
}

/// One entry in the shared priority queue.
///
/// A separate value from [`Pending`] because the heap holds **stale** entries by design: a
/// decrease-key is a second push, and the frontier map is the authority that makes the older
/// push a no-op when it pops. Rust's `BinaryHeap` has no `decrease_key`, and rebuilding the heap
/// on every relaxation would be quadratic.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    key: i64,
    repo: usize,
    oid: Oid,
}

impl Ord for Step {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Newest first, then the earlier repository in the scope's order, then the smaller oid.
        // A **total** order, and that is a requirement rather than a preference: two commits made
        // in the same second — a rebase writes a whole branch inside one — must not reshuffle
        // between two refreshes of the same query, because the user is looking at the list while
        // it happens.
        self.key
            .cmp(&other.key)
            .then_with(|| other.repo.cmp(&self.repo))
            .then_with(|| other.oid.as_bytes().cmp(self.oid.as_bytes()))
    }
}

impl PartialOrd for Step {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Memoised `Tree::get_path`, keyed by tree oid and path.
///
/// The value is `(blob oid, filemode)` and not just the blob oid, so that a bare `chmod +x`
/// counts as touching the path — which is what `git log -- path` does, and what a reader looking
/// for "when did this become executable" needs.
///
/// Its size is the memory cost of a path filter: at most one entry per (tree, tracked name) pair
/// the walk examined, so `scan_limit * MAX_TRACKED_PATHS` in the worst case.
type TreeCache = HashMap<(Oid, Rc<str>), Option<(Oid, i32)>>;

/// One repository's half of the walk. The heap is shared; nothing else is.
struct Walk<'r> {
    repo: &'r Repository,
    info: &'r RepoInfo,
    index: usize,
    frontier: IndexMap<Oid, Pending>,
    /// Commits already popped. Seeded from the resume token, so a second page cannot re-emit a
    /// row the first one drew.
    visited: HashSet<Oid>,
    trees: TreeCache,
    /// Parsed once from `<commondir>/shallow`. A commit in here has parents this clone does not
    /// contain; walking into them produces "object not found" rather than an end of history.
    shallow: HashSet<Oid>,
    chips: HashMap<Oid, Vec<RefChip>>,
    /// Cumulative across every page of this query, carried in the token — otherwise a filter
    /// that finds nothing is an unbounded walk taken one page at a time.
    scanned_total: u32,
    scanned_page: u32,
    /// The tail of the visited sequence, with the key each oid was seen at.
    ///
    /// Only the tail is kept, and that is sound rather than approximate: keys are non-increasing
    /// over the walk, so the oids that could still be re-reached from the frontier (`key <=` the
    /// frontier's maximum) are always a *suffix* of this sequence.
    seen_keys: VecDeque<(Oid, i64)>,
    recent: VecDeque<Oid>,
    watermark: i64,
    /// A stop this repository has already earned — `Shallow`, `NoSuchRef`, `CursorLost`. Sticky.
    stop: Option<LogStop>,
    /// libgit2's `slop` counter, live only when a range revspec gave us a hidden set.
    slop: i32,
    /// The last interesting commit's key — libgit2's `time` local in `limit_list`.
    last_key: i64,
}

/// A row on its way out, before the graph is laid over it.
struct Emitted {
    oid: Oid,
    parents: Vec<Oid>,
    row: CommitRow,
}

/// Everything about the query the per-commit code needs, bundled so the walk's signature stays
/// readable. Built once; the needles in it are lowercased once rather than per commit.
struct Params<'a> {
    query: &'a LogQuery,
    path: Option<&'a Rc<str>>,
    followed: bool,
    author: Option<Vec<u8>>,
    text: Option<Vec<u8>>,
    oid_prefix: Option<String>,
}

/// The mutable outputs a visit can add to.
struct Sink<'a> {
    heap: &'a mut BinaryHeap<Step>,
    renames: &'a mut Vec<RenameHop>,
    follow_capped: &'a mut bool,
    /// Pruned commit -> the parent it collapses into. Only ever written under
    /// [`Simplify::Default`] with a path filter, which is the one mode that rewrites edges, so it
    /// is empty for every unfiltered walk. Bounded by `scan_limit`.
    collapse: &'a mut HashMap<Oid, Oid>,
}

// --- entry point --------------------------------------------------------------------------------

/// One page of log, and a flag that stops it.
///
/// A struct rather than three positional arguments because the flag is the odd one out: `repos`
/// and `query` are the question, and `cancel` is a side channel into the loop. A caller that has
/// no side channel calls [`log`] instead and never sees this type.
pub struct LogWalk<'a> {
    /// The caller's resolved repository list — this crate knows nothing about projects, so the
    /// scope's [`RepoId`](cide_ipc::RepoId)s are looked up in it, and a scope naming a repository
    /// that is not there is [`GitError::NoSuchRepo`].
    pub repos: &'a [RepoInfo],
    pub query: &'a LogQuery,
    /// Set from any thread to stop the walk.
    ///
    /// Polled once per commit, at the top of the loop body — see [`log_cancellable`]. Borrowed
    /// and not owned: the registry that owns it lives in `cide-app`, per the module header.
    pub cancel: &'a AtomicBool,
}

/// One page of log.
///
/// [`log_cancellable`] over a flag nobody holds, kept as its own entry point so that the callers
/// with nothing to cancel — `cide-headless`, every test that is about the walk rather than about
/// stopping it — do not have to invent a flag to say they will never set one.
///
/// The flag is a **local** and not a `static`. A single process-wide `AtomicBool` shared by every
/// uncancellable caller would be one stray `store` away from stopping all of them at once, and
/// constructing an `AtomicBool` is a stack write.
pub fn log(repos: &[RepoInfo], query: &LogQuery) -> Result<CommitPage> {
    let never = AtomicBool::new(false);
    log_cancellable(&LogWalk {
        repos,
        query,
        cancel: &never,
    })
}

/// One page of log, stoppable partway.
///
/// Identical to [`log`] in every respect except that it polls [`LogWalk::cancel`] once per commit
/// and, when it is set, returns the rows found so far with [`CommitPage::cancelled`] — never an
/// error. `tests/log.rs::an_uncancelled_walk_is_byte_identical_to_logs_answer` is what pins "identical in
/// every respect" so the wrapper cannot quietly become a second walk.
pub fn log_cancellable(walk: &LogWalk<'_>) -> Result<CommitPage> {
    // Unpacked here and used by these names below, because `walk` is *also* what this function
    // calls one repository's half of the traversal ([`Walk`]) and the two must not read alike.
    let repos = walk.repos;
    let query = walk.query;
    let cancel = walk.cancel;

    let started = Instant::now();

    let limit = clamp_limit(query.limit) as usize;
    let scan_limit = clamp_scan(query.scan_limit);
    let selected = scope_repos(repos, &query.scope)?;
    let merged = matches!(query.scope, LogScope::Merged { .. });

    // The token is validated **before** anything is opened. A token from a different query would
    // build a frontier under one path filter and account the budget under another, and the
    // failure is not a crash — it is a log that quietly shows the wrong commits.
    let identity = query_identity(query);
    let resume = match &query.cursor {
        LogCursor::Resume { token } => {
            if token.version != TOKEN_VERSION || token.key != identity {
                return Err(GitError::StaleLogCursor);
            }
            Some(token)
        }
        _ => None,
    };

    // Every handle is opened here and dropped when this function returns — the crate's
    // no-cached-state rule, which the merged scope does not get to opt out of.
    let handles: Vec<Repository> = selected
        .iter()
        .map(|info| repo_mod::open(&info.root))
        .collect::<Result<Vec<_>>>()?;

    let path = query
        .path
        .as_deref()
        .filter(|p| !p.is_empty())
        .map(Rc::<str>::from);
    let text = needle(query.text.as_deref());
    let oid_prefix = text
        .as_ref()
        .filter(|n| n.len() >= 4 && n.iter().all(u8::is_ascii_hexdigit))
        .map(|n| String::from_utf8_lossy(n).into_owned());

    let mut heap: BinaryHeap<Step> = BinaryHeap::new();
    let mut walks: Vec<Walk<'_>> = Vec::with_capacity(selected.len());
    // `--follow` needs a single starting tip: with several, one commit is reachable under two
    // different names and the union of them is a claim neither tip supports. Reported as
    // `followed: false` rather than silently ignored, so a flag that did nothing is visible.
    let mut single_tip = true;

    for (index, (info, repo)) in selected.iter().zip(handles.iter()).enumerate() {
        let chips = ref_chips(repo)?;
        let mut walk = Walk {
            repo,
            info,
            index,
            frontier: IndexMap::new(),
            visited: HashSet::new(),
            trees: HashMap::new(),
            shallow: shallow_set(repo),
            chips,
            scanned_total: 0,
            scanned_page: 0,
            seen_keys: VecDeque::new(),
            recent: VecDeque::new(),
            watermark: 0,
            stop: None,
            slop: SLOP,
            last_key: i64::MAX,
        };

        match resume.and_then(|token| token.repos.iter().find(|r| r.repo == info.id)) {
            // A repository absent from the token starts from `Newest`. That is what makes
            // "open a second root while paging" produce a sane page instead of an empty one; a
            // *scope change* is a new query and mints a new token, so this arm is only ever
            // reached for a repository that genuinely has no position yet.
            Some(state) => seed_from_token(&mut walk, state, &mut heap, path.as_ref()),
            None => {
                let tips = seed_from_refs(&mut walk, query, path.as_ref())?;
                if tips != 1 {
                    single_tip = false;
                }
                for (oid, pending) in walk.frontier.iter() {
                    heap.push(Step {
                        key: pending.key,
                        repo: index,
                        oid: *oid,
                    });
                }
            }
        }
        walks.push(walk);
    }

    let params = Params {
        query,
        path: path.as_ref(),
        followed: query.follow && path.is_some() && single_tip,
        author: needle(query.author.as_deref()),
        text,
        oid_prefix,
    };

    let mut rows: Vec<Emitted> = Vec::new();
    let mut renames: Vec<RenameHop> = Vec::new();
    let mut follow_capped = false;
    let mut collapse: HashMap<Oid, Oid> = HashMap::new();
    let mut cursor_lost = false;

    // --- the seek, for `At` and `After` --------------------------------------------------------
    if let LogCursor::At { oid } | LogCursor::After { oid } = &query.cursor {
        let inclusive = matches!(query.cursor, LogCursor::At { .. });
        let target = Oid::from_str(oid).map_err(|_| GitError::NoSuchCommit { rev: oid.clone() })?;
        // Positioning is not searching: the commits the seek walks are reported in `scanned` but
        // must not eat the filter's budget, or "reveal this commit" on an old row would come back
        // as an empty `Budget` page having found exactly what it was asked for.
        let before: Vec<u32> = walks.iter().map(|w| w.scanned_total).collect();
        let mut sink = Sink {
            heap: &mut heap,
            renames: &mut renames,
            follow_capped: &mut follow_capped,
            collapse: &mut collapse,
        };
        let found = seek(&mut walks, &mut sink, &params, target, inclusive, cancel)?;
        for (walk, prior) in walks.iter_mut().zip(before) {
            walk.scanned_total = prior;
        }
        // `&& !cancel` is load-bearing. A cancelled seek stopped; it did not fail to find the
        // anchor, and it has proved nothing about whether the anchor is reachable. Letting it
        // fall into the arm below would mint `CursorLost`, whose entire meaning to the caller is
        // "throw your cursor away and restart from the newest commit" — so typing one more
        // character into the filter box while a reveal was in flight would jump the list to the
        // top of history. The walk loop polls the same flag on its first iteration and breaks, so
        // this path ends in an empty page marked `cancelled`, which is the honest answer.
        if !found && !cancel.load(Ordering::Acquire) {
            // The anchor is not reachable from these tips, or is further back than `MAX_SEEK`.
            // Not an error: a row the user clicked can be rewritten out of existence between the
            // click and the call, and the only useful answer is "restart from the top" — which is
            // exactly what `CursorLost` means.
            cursor_lost = true;
            for walk in &mut walks {
                walk.frontier.clear();
                walk.stop = Some(LogStop::CursorLost);
            }
            heap.clear();
        }
    }

    // --- the walk ------------------------------------------------------------------------------
    let mut budget_hit = false;
    let mut cancelled = false;
    while rows.len() < limit {
        // Once per commit, and **first**. Everything this iteration is about to do is more
        // expensive than the load: the parent lookups a path filter needs, `find_similar` on a
        // rename hop, the summary decode and `short_id`'s object-database round trip in
        // `commit_row`. Polling here rather than after them is the difference between a walk that
        // stops within one commit and one that stops within one commit's worth of work.
        //
        // Not every N commits, either. The load is uncontended and reads a line this thread
        // already owns, so it is lost in the noise of the work below; a stride counter would be a
        // second number to get wrong for a saving nobody can measure.
        if cancel.load(Ordering::Acquire) {
            cancelled = true;
            break;
        }
        let total: u32 = walks.iter().map(|w| w.scanned_total).sum();
        if total >= scan_limit {
            // `Budget`, emphatically **not** the end of history. A list that draws this as
            // `Exhausted` tells the user their file has no older revisions when it has thousands,
            // and silently loses the answer the whole budget design exists to keep findable.
            budget_hit = true;
            break;
        }
        let Some(step) = heap.pop() else { break };
        let walk = &mut walks[step.repo];
        let Some(pending) = take_current(walk, &step) else {
            continue;
        };
        let mut sink = Sink {
            heap: &mut heap,
            renames: &mut renames,
            follow_capped: &mut follow_capped,
            collapse: &mut collapse,
        };
        if let Some(row) = visit(walk, &step, &pending, &params, &mut sink)? {
            rows.push(row);
        }
    }

    // --- rewrite the edges, then lay out the graph ----------------------------------------------
    let emitted: HashSet<Oid> = rows.iter().map(|r| r.oid).collect();
    let mut rewritten: Vec<(Vec<Oid>, Vec<bool>)> = Vec::with_capacity(rows.len());
    for row in &mut rows {
        let (parents, dangling, pruned) = rewrite(&collapse, &emitted, &row.parents);
        row.row.pruned = pruned;
        rewritten.push((parents, dangling));
    }

    let mut lane_state: Option<LaneResume> = None;
    let graph = match graph_off(query, merged, resume) {
        Some(reason) => LogGraph::Off { reason },
        None => {
            let mut lanes = Lanes::resume(
                resume.and_then(|t| t.lanes.as_ref()),
                query.graph_lanes.max(1),
            );
            let mut out = Vec::with_capacity(rows.len());
            for (row, (parents, dangling)) in rows.iter().zip(rewritten.iter()) {
                out.push(lanes.row(row.oid, parents, dangling));
            }
            let width = lanes.width();
            let overflow = lanes.overflowed();
            lane_state = Some(lanes.finish());
            LogGraph::Rows {
                rows: out,
                lanes: width,
                overflow,
            }
        }
    };

    // --- stops, per repository and then composed ------------------------------------------------
    let repo_pages: Vec<RepoPage> = walks
        .iter()
        .map(|walk| RepoPage {
            repo: walk.info.id,
            name: walk.info.name.clone(),
            stop: repo_stop(walk, budget_hit),
            scanned: walk.scanned_page,
        })
        .collect();
    // `LogScope::One` is the fast path: with one repository there is nothing to compose, and
    // taking the maximum over a one-element list is the same answer arrived at more slowly.
    let stop = if repo_pages.len() == 1 {
        repo_pages[0].stop
    } else {
        repo_pages
            .iter()
            .map(|p| p.stop)
            .max_by_key(|s| stop_rank(*s))
            .unwrap_or(LogStop::Exhausted)
    };

    let scanned: u32 = walks.iter().map(|w| w.scanned_page).sum();
    let resume_token = build_resume(&walks, &identity, lane_state, cursor_lost);
    let commits: Vec<CommitRow> = rows.into_iter().map(|r| r.row).collect();

    tracing::debug!(
        scanned,
        matched = commits.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        ?stop,
        cancelled,
        "git log page"
    );

    Ok(CommitPage {
        commits,
        graph,
        resume: resume_token,
        repos: repo_pages,
        stop,
        scanned,
        // True only when the loop above actually saw the flag. A page that filled its `limit`
        // and *then* had the flag set reports `false`, and that is right: it is a complete page,
        // and there is nothing partial about it for the caller to discard.
        cancelled,
        renames,
        followed: params.followed,
        follow_capped,
    })
}

// --- one commit ----------------------------------------------------------------------------------

/// Pop the frontier entry a heap step refers to, or `None` when the step is stale.
///
/// The map is the authority. A decrease-key pushed a second `Step` with the smaller key; the
/// larger one is still in the heap, pops first, and is dropped here.
fn take_current(walk: &mut Walk<'_>, step: &Step) -> Option<Pending> {
    let pending = walk.frontier.get(&step.oid)?;
    if pending.key != step.key {
        return None;
    }
    walk.frontier.shift_remove(&step.oid)
}

fn visit(
    walk: &mut Walk<'_>,
    step: &Step,
    pending: &Pending,
    params: &Params<'_>,
    sink: &mut Sink<'_>,
) -> Result<Option<Emitted>> {
    // Copied out of the struct so the `&mut Walk` borrows below (the tree cache) and the object
    // lookups do not fight: the handle outlives the walk, the caches do not.
    let repo = walk.repo;

    walk.visited.insert(step.oid);
    walk.scanned_total += 1;
    walk.scanned_page += 1;
    remember(walk, step.oid, step.key);

    let commit = match repo.find_commit(step.oid) {
        Ok(commit) => commit,
        // A frontier oid that no longer resolves is the whole reason `CursorLost` exists. It
        // happens for real: a `gc` after a rebase removes commits a token from ninety seconds ago
        // still names.
        Err(_) => {
            walk.stop = Some(LogStop::CursorLost);
            return Ok(None);
        }
    };

    let parents: Vec<Oid> = commit.parent_ids().collect();
    let tree = commit.tree_id();

    // --- the uninteresting side of a range -------------------------------------------------------
    if !pending.interesting {
        // libgit2's `add_parents_to_list`, uninteresting branch (`revwalk.c:395-412`): "go full on
        // in the uninteresting case as we want to include as many of these as we can". Note that
        // it does **not** honour `first_parent` here — the point is to hide as much as possible,
        // and a half-hidden side of a merge leaves rows in `a..b` that `git rev-list` would not
        // print.
        for parent in &parents {
            push_parent(walk, sink.heap, *parent, step.key, &pending.paths, false);
        }
        let max_key = walk.frontier.values().map(|p| p.key).max();
        walk.slop = still_interesting(&walk.frontier, max_key, walk.last_key, walk.slop);
        if walk.slop == 0 {
            // The interesting side is exhausted and the slop is spent. Everything left on this
            // repository's frontier is uninteresting, and dropping it is what ends `a..b`.
            walk.frontier.clear();
        }
        return Ok(None);
    }
    walk.last_key = step.key;

    // --- does this commit touch the tracked path? -------------------------------------------------
    //
    // A tree-entry comparison, never a per-commit diff. `Tree::get_path` memoised by tree oid
    // costs one lookup per (tree, path) pair and answers the question exactly; a
    // `diff_tree_to_tree` per commit runs xdiff over every changed file to answer a question
    // about one entry. A directory resolves to its subtree oid, so history on a folder is the
    // same code with no special case, and `(blob oid, filemode)` rather than the oid alone is
    // what makes a bare `chmod +x` count.
    //
    // The rule is git's own, and **the two simplification modes do not share it** — which is the
    // single easiest thing here to get wrong, because the difference only shows on a merge:
    //
    // ```
    // parents == 0  ->  entry(commit).is_some()                      (both modes)
    // Default       ->  entry(commit) != entry(p) for EVERY p
    // Full          ->  entry(commit) != entry(p) for AT LEAST ONE p
    // ```
    //
    // `Default` is git's "is this merge interesting" test. A merge whose result matches *some*
    // parent brought the file along and did nothing to it; a merge that differs from every parent
    // is an **evil merge** — a change that exists on no side — and it is the one merge a file's
    // history must show. For a single-parent commit the clause degenerates to the obvious one.
    //
    // `Full` is `--full-history`, and it lists strictly more: git's `try_to_simplify_commit`
    // only marks a commit TREESAME (and prunes its parents to the matching one) when
    // `revs->simplify_history` is set, which `--full-history` clears — so a merge that matches one
    // parent and differs from another stays in the list. Verified against the binary rather than
    // read off the documentation, whose worked example only covers the TREESAME-to-*both* case:
    // `tests/log.rs::a_merge_is_listed_only_when_it_differs_from_every_parent` and
    // `simplify_default_and_full_are_gits_two_answers` are that check.
    //
    // Under `Full` we do **no** parent rewriting, so the rows are a subsequence of history and the
    // graph is switched off with `GraphOff::FullHistory`. Under `Default` this test decides
    // listing, the rule below decides traversal, and the edges are rewritten to match.
    let mut treesame_parent: Option<usize> = None;
    let touched = if params.path.is_none() {
        true
    } else if parents.is_empty() {
        let mut exists = false;
        for tracked in &pending.paths {
            if entry_at(walk, tree, tracked)?.is_some() {
                exists = true;
                break;
            }
        }
        exists
    } else {
        let mut differs_from_any = false;
        for (index, parent) in parents.iter().enumerate() {
            let mut same = true;
            for tracked in &pending.paths {
                if entry_at(walk, tree, tracked)? != parent_entry(walk, *parent, tracked)? {
                    same = false;
                    break;
                }
            }
            if same {
                treesame_parent.get_or_insert(index);
            } else {
                differs_from_any = true;
            }
        }
        match params.query.simplify {
            Simplify::Default => treesame_parent.is_none(),
            Simplify::Full => differs_from_any,
        }
    };

    // --- which parents to follow ------------------------------------------------------------------
    let follow_indices: Vec<usize> = if params.query.first_parent {
        // `--first-parent`: the sequence of things that landed on this branch. Overrides
        // simplification's choice of parent, exactly as git's flag does.
        if parents.is_empty() {
            Vec::new()
        } else {
            vec![0]
        }
    } else if params.path.is_some() && params.query.simplify == Simplify::Default {
        match treesame_parent {
            // git: "If the commit was a merge, and it was TREESAME to one parent, follow only
            // that parent. (Even if there are several TREESAME parents, follow only one of them.)"
            Some(index) => vec![index],
            None => (0..parents.len()).collect(),
        }
    } else {
        (0..parents.len()).collect()
    };

    // --- rename hops -------------------------------------------------------------------------------
    //
    // The tracked path is a property of the frontier entry, so a hop rewrites what the *parent* is
    // looking for and leaves every other lane alone. Hops over the walk are uncapped — one fires
    // per real rename — and it is the cost of a hop that is bounded.
    let mut parent_paths: Vec<Vec<Rc<str>>> = vec![pending.paths.clone(); parents.len()];
    if params.followed && !parents.is_empty() {
        let mut hops = 0usize;
        for &index in &follow_indices {
            let parent = parents[index];
            let mut names = pending.paths.clone();
            for slot in 0..names.len() {
                if hops >= MAX_FOLLOW_HOPS {
                    *sink.follow_capped = true;
                    break;
                }
                let tracked = names[slot].clone();
                let here = entry_at(walk, tree, &tracked)?;
                let there = parent_entry(walk, parent, &tracked)?;
                // A hop is only possible where the name *appears* at this commit and does not
                // exist in the parent at all. An ordinary modification is not a rename and must
                // not pay for a whole-tree diff.
                if here.is_none() || there.is_some() {
                    continue;
                }
                walk.scanned_total += RENAME_SCAN_COST;
                walk.scanned_page += RENAME_SCAN_COST;
                hops += 1;
                if let Some((from, similarity)) =
                    detect_rename(repo, parent, tree, &tracked, sink.follow_capped)?
                {
                    sink.renames.push(RenameHop {
                        repo: walk.info.id,
                        oid: step.oid.to_string(),
                        from: from.clone(),
                        to: tracked.to_string(),
                        similarity,
                    });
                    let from: Rc<str> = Rc::from(from.as_str());
                    if !names.contains(&from) {
                        names[slot] = from;
                    }
                }
            }
            if names.len() > MAX_TRACKED_PATHS {
                names.truncate(MAX_TRACKED_PATHS);
                *sink.follow_capped = true;
            }
            parent_paths[index] = names;
        }
    }

    // --- push the parents --------------------------------------------------------------------------
    //
    // A shallow graft is where this clone's history *stops existing*, not where the project began.
    // Walking into it produces "object not found"; reporting it as `Exhausted` tells the user
    // their project started at a commit from last Tuesday.
    if walk.shallow.contains(&step.oid) {
        walk.stop = Some(LogStop::Shallow);
    } else {
        for &index in &follow_indices {
            push_parent(
                walk,
                sink.heap,
                parents[index],
                step.key,
                &parent_paths[index],
                true,
            );
        }
    }

    if !touched {
        // Simplification suppresses the row **and** rewrites the edge. Recording where this commit
        // collapses to is what lets a child's lane be re-pointed at a row that is actually in the
        // list — git's `rewrite_one`, resolved in emission order once the page is known.
        if let Some(&index) = follow_indices.first() {
            sink.collapse.insert(step.oid, parents[index]);
        }
        return Ok(None);
    }

    // --- the filters -------------------------------------------------------------------------------
    //
    // Here, and not before the parents were pushed. **A filter suppresses the row and never the
    // traversal**: a commit whose message does not mention the ticket is very often the child of
    // forty that do, and pruning here would make all forty vanish.
    //
    // In Rust and not in the component, because a hundred-thousand-commit list must not cross the
    // IPC wire to be `.filter()`ed in TypeScript — that is the whole reason `scan_limit` is a
    // *server* budget.
    let author = commit.author();
    if let Some(needle) = &params.author {
        let name = author.name_bytes();
        let email = author.email_bytes();
        if !contains_ci(name, needle) && !contains_ci(email, needle) {
            return Ok(None);
        }
    }
    if let Some(needle) = &params.text {
        // An oid prefix is a text search people type constantly — they paste a short hash into the
        // filter box — and it would otherwise match nothing, because the hash is not in the
        // message. Four characters is git's own floor for an abbreviation.
        let by_oid = params
            .oid_prefix
            .as_deref()
            .is_some_and(|prefix| step.oid.to_string().starts_with(prefix));
        if !by_oid && !contains_ci(commit.message_bytes(), needle) {
            return Ok(None);
        }
    }

    walk.watermark = step.key;
    let row = commit_row(walk.info.id, &commit, &walk.chips);

    Ok(Some(Emitted {
        oid: step.oid,
        parents,
        row,
    }))
}

/// Build the row for one commit.
///
/// Public and shared with [`crate::show`], because the detail pane draws the *same* row the list
/// drew and the two must not be able to disagree about a summary, an abbreviation or a ref chip —
/// which is exactly what [`cide_ipc::history::CommitDetail::commit`] being a whole [`CommitRow`]
/// is for.
pub fn commit_row(
    repo: cide_ipc::RepoId,
    commit: &git2::Commit<'_>,
    chips: &HashMap<Oid, Vec<RefChip>>,
) -> CommitRow {
    let oid = commit.id();
    let author = commit.author();
    CommitRow {
        repo,
        oid: oid.to_string(),
        // libgit2's own uniqueness rule, so the abbreviation cannot disagree with what `git log`
        // prints in the same repository. The fallback is only reached when the object database
        // cannot be queried at all, which is a state in which eight characters is the least of
        // anyone's problems.
        short_oid: commit
            .as_object()
            .short_id()
            .ok()
            .and_then(|buf| buf.as_str().ok().map(str::to_owned))
            .unwrap_or_else(|| oid.to_string().chars().take(8).collect()),
        summary: commit
            .summary_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default(),
        // Lossy rather than refused: a commit whose author name has one Latin-1 byte in it is
        // still a row someone needs to see, and the replacement character says where the problem
        // is. `branch::collect` refuses instead, and it is right to — a *checkout* acts on the
        // name, a log row only shows it.
        author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
        author_email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
        authored: author.when().seconds(),
        committed: commit.time().seconds(),
        parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        pruned: 0,
        refs: chips.get(&oid).cloned().unwrap_or_default(),
    }
}

/// Queue a parent, unioning its tracked paths when it is already there.
fn push_parent(
    walk: &mut Walk<'_>,
    heap: &mut BinaryHeap<Step>,
    parent: Oid,
    child_key: i64,
    paths: &[Rc<str>],
    interesting: bool,
) {
    let repo = walk.repo;
    if interesting && walk.visited.contains(&parent) {
        return;
    }
    let time = match repo.find_commit(parent) {
        Ok(commit) => commit.time().seconds(),
        // A parent that is not in the object database is a graft this clone did not record in
        // `.git/shallow` — an old clone, a partial fetch, a filtered mirror. Same answer: the
        // history stops here, and it must not be called the end.
        Err(_) => {
            walk.stop = Some(LogStop::Shallow);
            return;
        }
    };
    // The clamp. See `Pending::key`; this one line is what makes paging exact.
    let key = time.min(child_key);
    let index = walk.index;

    match walk.frontier.get_mut(&parent) {
        Some(existing) => {
            for path in paths {
                if !existing.paths.contains(path) && existing.paths.len() < MAX_TRACKED_PATHS {
                    existing.paths.push(path.clone());
                }
            }
            // Uninteresting always wins, and it wins retroactively while the commit is still on
            // the frontier — libgit2 sets `p->uninteresting = 1` regardless of `seen`.
            if !interesting {
                existing.interesting = false;
            }
            if key < existing.key {
                // Decrease-key. `BinaryHeap` has none, so the relaxation is a second push and the
                // older entry is dropped when it pops — `take_current` is that check.
                existing.key = key;
                heap.push(Step {
                    key,
                    repo: index,
                    oid: parent,
                });
            }
        }
        None => {
            if walk.visited.contains(&parent) {
                // Already emitted, and only now discovered to be uninteresting. Too late to
                // un-emit it: this is the streaming approximation libgit2 sidesteps by running
                // `limit_list` to completion before yielding anything, and it is exactly what SLOP
                // bounds.
                return;
            }
            walk.frontier.insert(
                parent,
                Pending {
                    key,
                    paths: paths.to_vec(),
                    interesting,
                },
            );
            heap.push(Step {
                key,
                repo: index,
                oid: parent,
            });
        }
    }
}

/// Port of libgit2's `still_interesting` (`revwalk.c:443-467`), verbatim in structure.
///
/// The rule it encodes is not obvious and is not ours to improve on: after the interesting side of
/// a range is exhausted, keep walking a few more uninteresting commits, because a commit reached
/// only through an uninteresting one may still turn out to be interesting through another edge.
/// Cutting the moment the last interesting commit pops drops the far side of every merge in the
/// range. `SLOP` is five in git and five here for the same reason — it is a compromise nobody has
/// found a principled replacement for.
///
/// `max_key` stands in for libgit2's `list->item->time`, the head of its date-ordered list, which
/// is our frontier's maximum.
fn still_interesting(
    frontier: &IndexMap<Oid, Pending>,
    max_key: Option<i64>,
    time: i64,
    slop: i32,
) -> i32 {
    // "The empty list is pretty boring."
    let Some(max_key) = max_key else { return 0 };
    // The frontier holds something newer than the last row emitted, so the walk is not done.
    if time <= max_key {
        return SLOP;
    }
    for pending in frontier.values() {
        if pending.interesting || pending.key > time {
            return SLOP;
        }
    }
    slop - 1
}

// --- tree entries ----------------------------------------------------------------------------------

fn entry_at(walk: &mut Walk<'_>, tree: Oid, path: &Rc<str>) -> Result<Option<(Oid, i32)>> {
    if let Some(hit) = walk.trees.get(&(tree, path.clone())) {
        return Ok(*hit);
    }
    let found = match walk.repo.find_tree(tree) {
        Ok(tree) => tree
            .get_path(Path::new(&**path))
            .ok()
            .map(|entry| (entry.id(), entry.filemode())),
        Err(_) => None,
    };
    walk.trees.insert((tree, path.clone()), found);
    Ok(found)
}

fn parent_entry(walk: &mut Walk<'_>, parent: Oid, path: &Rc<str>) -> Result<Option<(Oid, i32)>> {
    let tree = match walk.repo.find_commit(parent) {
        Ok(commit) => commit.tree_id(),
        // A missing parent (a graft) behaves as the empty tree: everything in the child is an
        // addition against it, which is what `git log` shows at a shallow floor.
        Err(_) => return Ok(None),
    };
    entry_at(walk, tree, path)
}

// --- rename detection --------------------------------------------------------------------------------

/// Find what `tracked` was called in `parent`.
///
/// # Why the diff is not pathspec-limited
///
/// This is the trap `crates/cide-app/src/cmd/git.rs:220-231` already documents from the other
/// side. libgit2 applies a pathspec **while building the diff**, so a pathspec of the new name
/// leaves `find_similar` holding one side of the rename with nothing to match it against — it
/// reports `Added`, and the follow stops at exactly the rename it exists for. A whole-tree diff is
/// the only shape that can detect a rename at all, and that is why a hop costs
/// [`RENAME_SCAN_COST`] rather than one commit.
///
/// The cost is bounded twice: [`RENAME_MAX_DELTAS`] refuses the pass outright on a tree-wide move,
/// where `find_similar` is quadratic in the unmatched adds and deletes, and libgit2's own
/// `rename_limit` bounds the matrix it will build.
fn detect_rename(
    repo: &Repository,
    parent: Oid,
    tree: Oid,
    tracked: &Rc<str>,
    capped: &mut bool,
) -> Result<Option<(String, u16)>> {
    let Ok(parent_tree) = repo.find_commit(parent).and_then(|c| c.tree()) else {
        return Ok(None);
    };
    let Ok(new_tree) = repo.find_tree(tree) else {
        return Ok(None);
    };
    let mut opts = git2::DiffOptions::new();
    opts.include_typechange(true);
    let mut diff = repo
        .diff_tree_to_tree(Some(&parent_tree), Some(&new_tree), Some(&mut opts))
        .wrap()?;
    if diff.deltas().len() > RENAME_MAX_DELTAS {
        *capped = true;
        return Ok(None);
    }
    let mut find = git2::DiffFindOptions::new();
    find.renames(true).copies(false).rename_limit(1_000);
    diff.find_similar(Some(&mut find)).wrap()?;

    let mut from: Option<String> = None;
    for delta in diff.deltas() {
        if delta.status() != git2::Delta::Renamed {
            continue;
        }
        if path_of(&delta.new_file()).as_deref() != Some(&**tracked) {
            continue;
        }
        from = path_of(&delta.old_file());
        break;
    }
    let Some(from) = from else { return Ok(None) };

    // git2 0.21 does not expose `git_diff_delta.similarity` — `git2/src/diff.rs:521` has it
    // commented out as "TODO: expose when diffs are more exposed" — and this crate does not depend
    // on `libgit2-sys`, so the number cannot be read off the struct. It *is* printed, by
    // `diff_print.c:390`, in the patch **header**, and `DiffFormat::PatchHeader` renders headers
    // and nothing else, so this costs no xdiff. Parsing libgit2's own output beats inventing a
    // second similarity metric here, which would disagree with `git diff` on exactly the
    // borderline renames the number is for.
    let mut similarity = 0u16;
    let mut ours = false;
    let _ = diff.print(git2::DiffFormat::PatchHeader, |delta, _, line| {
        if line.origin() == 'F' {
            ours = path_of(&delta.new_file()).as_deref() == Some(&**tracked);
        }
        if ours && let Ok(text) = std::str::from_utf8(line.content()) {
            for entry in text.lines() {
                if let Some(rest) = entry.strip_prefix("similarity index ")
                    && let Some(number) = rest.strip_suffix('%')
                    && let Ok(parsed) = number.parse::<u16>()
                {
                    similarity = parsed.min(100);
                }
            }
        }
        true
    });

    Ok(Some((from, similarity)))
}

fn path_of(file: &git2::DiffFile<'_>) -> Option<String> {
    file.path_bytes()
        .map(|b| String::from_utf8_lossy(b).into_owned())
}

// --- filters -----------------------------------------------------------------------------------------

/// Lowercase the needle once. Lowercasing the *haystack* per commit would allocate a copy of every
/// commit message in the repository to answer one substring question.
fn needle(value: Option<&str>) -> Option<Vec<u8>> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_ascii_lowercase().into_bytes())
}

/// ASCII-case-insensitive substring search.
///
/// ASCII folding and not Unicode folding, on purpose: full folding needs the haystack decoded and
/// normalised, which is a per-commit allocation, and what people type into a log filter is a
/// ticket number, a path or a name. Written down here rather than discovered later by someone
/// wondering why `Ä` does not match `ä`.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

// --- edges --------------------------------------------------------------------------------------------

/// Resolve a row's real parents through the collapse map onto rows that are actually in the page,
/// and count the hops.
///
/// [`CommitRow::parents`] keeps the **real** parents — that is a fact about the object, and a
/// caller passing one to `git show` must get the answer git gives. The rewritten pair is for the
/// graph only, where an edge into a row that is not in the list is the bug.
fn rewrite(
    collapse: &HashMap<Oid, Oid>,
    emitted: &HashSet<Oid>,
    parents: &[Oid],
) -> (Vec<Oid>, Vec<bool>, u32) {
    let mut out: Vec<Oid> = Vec::with_capacity(parents.len());
    let mut dangling: Vec<bool> = Vec::with_capacity(parents.len());
    let mut pruned = 0u32;
    // A chain cannot cycle in a DAG, but the bound costs nothing and turns a corrupted map into a
    // wrong graph rather than a hang.
    let ceiling = collapse.len() + 1;
    for parent in parents {
        let mut current = *parent;
        let mut hops = 0usize;
        while !emitted.contains(&current) && hops < ceiling {
            match collapse.get(&current) {
                Some(next) => {
                    current = *next;
                    hops += 1;
                }
                None => break,
            }
        }
        pruned += hops as u32;
        // Deduplicate: two real parents can collapse onto one surviving ancestor, and two edges
        // into one lane draw as a doubled line.
        if out.contains(&current) {
            continue;
        }
        dangling.push(!emitted.contains(&current));
        out.push(current);
    }
    (out, dangling, pruned)
}

/// Why this page has no graph, or `None` when it has one.
///
/// The order is the order of certainty, not of importance: `Disabled` is what the caller
/// explicitly asked for, `Merged` is a fact about the scope, and the rest are facts about the row
/// set. Two of them can be true at once and the panel shows one sentence.
///
/// **A path filter under [`Simplify::Default`] keeps its graph**, and that is not an oversight:
/// the parents are rewritten onto surviving ancestors, so the row set *is* closed under the
/// rewritten parent function. It is why `git log --graph -- path` works, and why
/// `git log --graph --full-history -- path` is a mesh.
fn graph_off(query: &LogQuery, merged: bool, resume: Option<&LogResume>) -> Option<GraphOff> {
    if !query.graph {
        return Some(GraphOff::Disabled);
    }
    if merged {
        // Two repositories share no DAG. A line crossing hundreds of foreign rows is worse than no
        // line; the repo chip on the row carries the information instead.
        return Some(GraphOff::Merged);
    }
    if query.author.is_some() || query.text.is_some() {
        // The rows are a *subsequence*: the row above is not the child of the row below, it is
        // merely the next match. Inventing "nearest surviving ancestor" here would assert a
        // reachability nothing computed.
        return Some(GraphOff::Filtered);
    }
    if query.simplify == Simplify::Full && query.path.is_some() {
        return Some(GraphOff::FullHistory);
    }
    if matches!(query.cursor, LogCursor::At { .. } | LogCursor::After { .. })
        && resume.and_then(|t| t.lanes.as_ref()).is_none()
    {
        // The first rows' parents are off the top of the page and their columns cannot be joined
        // to anything. Drawing them anyway gives a graph whose first screen is wrong and then
        // silently becomes right, which is worse than no graph at all.
        return Some(GraphOff::Rerooted);
    }
    None
}

// --- seeding ------------------------------------------------------------------------------------------

/// Push the tips [`LogRefs`] names, and hide what a range hides. Returns how many tips were pushed.
fn seed_from_refs(walk: &mut Walk<'_>, query: &LogQuery, path: Option<&Rc<str>>) -> Result<usize> {
    let paths: Vec<Rc<str>> = path.cloned().into_iter().collect();
    let mut tips: Vec<Oid> = Vec::new();
    let mut hidden: Vec<Oid> = Vec::new();

    match &query.refs {
        LogRefs::Head => {
            // An unborn HEAD is a repository with no commits: a normal state, an empty page, and
            // emphatically not an error. A new project must be able to open its log without a
            // dialog.
            if let Some(commit) = walk.repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
                tips.push(commit.id());
            }
        }
        LogRefs::Branch { name } => match find_branch_tip(walk.repo, name) {
            Some(oid) => tips.push(oid),
            // A view pinned to a branch someone has since deleted says so in the panel; it does
            // not raise a dialog. Hence a stop and not an error.
            None => walk.stop = Some(LogStop::NoSuchRef),
        },
        LogRefs::All => {
            tips.extend(walk.chips.keys().copied());
            if let Some(commit) = walk.repo.head().ok().and_then(|h| h.peel_to_commit().ok())
                && !tips.contains(&commit.id())
            {
                tips.push(commit.id());
            }
            // A total order over the tips, so `--all` opens its lanes in the same order every
            // time and the graph does not recolour itself between two identical requests.
            tips.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        }
        LogRefs::Rev { spec } => {
            let (push, hide) = resolve_revspec(walk.repo, spec)?;
            tips = push;
            hidden = hide;
        }
    }

    for oid in &hidden {
        if let Ok(commit) = walk.repo.find_commit(*oid) {
            walk.frontier.insert(
                *oid,
                Pending {
                    key: commit.time().seconds(),
                    paths: paths.clone(),
                    interesting: false,
                },
            );
        }
    }
    let mut pushed = 0;
    for oid in &tips {
        let Ok(commit) = walk.repo.find_commit(*oid) else {
            continue;
        };
        // Hidden wins over pushed: `a..a` is empty, not "a and everything under it".
        if walk.frontier.contains_key(oid) {
            continue;
        }
        walk.frontier.insert(
            *oid,
            Pending {
                key: commit.time().seconds(),
                paths: paths.clone(),
                interesting: true,
            },
        );
        pushed += 1;
    }
    Ok(pushed)
}

/// `refs/heads/<name>` first, then `refs/remotes/<name>` — the same order the branch selector
/// resolves in, so a chip labelled `main` and a filter typed as `main` cannot mean two things.
fn find_branch_tip(repo: &Repository, name: &str) -> Option<Oid> {
    for kind in [git2::BranchType::Local, git2::BranchType::Remote] {
        if let Ok(branch) = repo.find_branch(name, kind)
            && let Ok(commit) = branch.get().peel_to_commit()
        {
            return Some(commit.id());
        }
    }
    None
}

/// Resolve a revspec into (tips to push, tips to hide).
///
/// `Repository::revparse`, and **not** a classification of the string here: `RevparseMode` is
/// libgit2's own answer to single / range / merge-base, and re-deriving it would be
/// reimplementing git's parser — which gets `HEAD@{2}`, `:/fix typo` and `v1.2^{}` wrong in ways
/// that only surface on somebody else's repository.
fn resolve_revspec(repo: &Repository, spec: &str) -> Result<(Vec<Oid>, Vec<Oid>)> {
    let parsed = repo.revparse(spec).map_err(|e| match e.code() {
        // "type more characters" is a specific, mechanical fix, and deserves its own arm rather
        // than being buried in a parse error.
        git2::ErrorCode::Ambiguous => GitError::AmbiguousRev {
            spec: spec.to_string(),
        },
        _ => GitError::BadRevspec {
            spec: spec.to_string(),
            detail: e.message().to_string(),
        },
    })?;
    let mode = parsed.mode();

    let peel = |object: Option<&git2::Object<'_>>| -> Result<Option<Oid>> {
        let Some(object) = object else {
            return Ok(None);
        };
        match object.peel_to_commit() {
            Ok(commit) => Ok(Some(commit.id())),
            // The `HEAD^{tree}` case: it parses, it resolves, and *then* it fails to peel.
            // libgit2's message for that says nothing about what to type instead, so the kind is
            // carried and the sentence is the frontend's to write.
            Err(_) => Err(GitError::NotACommit {
                spec: spec.to_string(),
                kind: object
                    .kind()
                    .map(|k| k.str().to_string())
                    .unwrap_or_else(|| "object".into()),
            }),
        }
    };

    let from = peel(parsed.from())?;
    let to = peel(parsed.to())?;

    if mode.contains(RevparseMode::MERGE_BASE) {
        // `a...b`: everything reachable from either, minus everything reachable from both. Every
        // merge base is hidden, not just the first — `merge_base` alone is wrong for a criss-cross
        // history, which is precisely where a symmetric difference is interesting.
        let (Some(from), Some(to)) = (from, to) else {
            return Err(GitError::BadRevspec {
                spec: spec.to_string(),
                detail: "a symmetric difference needs both sides".into(),
            });
        };
        let bases = repo
            .merge_bases(from, to)
            .map(|a| a.to_vec())
            .unwrap_or_default();
        return Ok((vec![from, to], bases));
    }
    if mode.contains(RevparseMode::RANGE) {
        // `a..b`: reachable from b and not from a. An **empty** range is zero rows and
        // `Exhausted`, never an error — `main..main` is a question whose answer is "nothing".
        let Some(to) = to else {
            return Err(GitError::BadRevspec {
                spec: spec.to_string(),
                detail: "a range needs a right-hand side".into(),
            });
        };
        return Ok((vec![to], from.into_iter().collect()));
    }
    Ok((from.into_iter().collect(), Vec::new()))
}

/// Re-seed one repository's frontier from a resume token.
fn seed_from_token(
    walk: &mut Walk<'_>,
    state: &RepoResume,
    heap: &mut BinaryHeap<Step>,
    path: Option<&Rc<str>>,
) {
    walk.scanned_total = state.scanned;
    walk.watermark = state.watermark;
    for oid in state.hidden.iter().chain(state.recent.iter()) {
        if let Ok(oid) = Oid::from_str(oid) {
            walk.visited.insert(oid);
        }
    }
    if state.done {
        return;
    }
    for entry in &state.frontier {
        let Ok(oid) = Oid::from_str(&entry.oid) else {
            walk.stop = Some(LogStop::CursorLost);
            walk.frontier.clear();
            return;
        };
        if walk.repo.find_commit(oid).is_err() {
            // A rewrite, a prune or a `gc` between two pages. A *partial* frontier would walk a
            // subset of the history and report it as the whole, so the honest answer is to refuse
            // the whole continuation and let the caller restart from `Newest`.
            walk.stop = Some(LogStop::CursorLost);
            walk.frontier.clear();
            return;
        }
        let paths: Vec<Rc<str>> = if entry.paths.is_empty() {
            path.cloned().into_iter().collect()
        } else {
            entry.paths.iter().map(|p| Rc::from(p.as_str())).collect()
        };
        walk.frontier.insert(
            oid,
            Pending {
                key: entry.key,
                paths,
                interesting: true,
            },
        );
        heap.push(Step {
            key: entry.key,
            repo: walk.index,
            oid,
        });
    }
}

// --- the seek --------------------------------------------------------------------------------------------

/// Walk without emitting until `target` pops. `false` when it was not reachable within
/// [`MAX_SEEK`].
///
/// Parents are pushed under the same simplification and follow rules the real walk uses, because a
/// frontier built by a *different* rule is a frontier the page after this one would walk wrongly.
///
/// Cancellable, and it is the one loop here that most needs to be: [`MAX_SEEK`] is deliberately
/// twice [`SCAN_MAX`], so this is the longest-running loop in the module, and a cancel that could
/// not reach it would be a control that does nothing for exactly the request that runs longest.
///
/// Its `false` therefore means **stopped**, which is not the same claim as "not there". The caller
/// re-reads the flag before it draws any conclusion about the anchor; see the call site.
fn seek(
    walks: &mut [Walk<'_>],
    sink: &mut Sink<'_>,
    params: &Params<'_>,
    target: Oid,
    inclusive: bool,
    cancel: &AtomicBool,
) -> Result<bool> {
    let mut used = 0u32;
    while used < MAX_SEEK {
        if cancel.load(Ordering::Acquire) {
            return Ok(false);
        }
        let Some(step) = sink.heap.pop() else {
            return Ok(false);
        };
        if step.oid == target && inclusive {
            // Put it back: the real loop pops it again and emits it. Re-pushing rather than
            // emitting here keeps exactly one copy of the row-building code.
            sink.heap.push(step);
            return Ok(true);
        }
        let walk = &mut walks[step.repo];
        let Some(pending) = take_current(walk, &step) else {
            continue;
        };
        used += 1;
        let hit = step.oid == target;
        visit(walk, &step, &pending, params, sink)?;
        if hit {
            return Ok(true);
        }
    }
    Ok(false)
}

// --- stops and the token ----------------------------------------------------------------------------------

/// Most restrictive first: `CursorLost > Budget > Shallow > NoSuchRef > Page > Exhausted`.
fn stop_rank(stop: LogStop) -> u8 {
    match stop {
        LogStop::Exhausted => 0,
        LogStop::Page => 1,
        LogStop::NoSuchRef => 2,
        LogStop::Shallow => 3,
        LogStop::Budget => 4,
        LogStop::CursorLost => 5,
    }
}

fn repo_stop(walk: &Walk<'_>, budget_hit: bool) -> LogStop {
    if walk.stop == Some(LogStop::CursorLost) {
        return LogStop::CursorLost;
    }
    // The budget outranks a shallow floor: having run out of budget, we do not actually know
    // whether the graft was the end of what we would have shown.
    if budget_hit && !walk.frontier.is_empty() {
        return LogStop::Budget;
    }
    if let Some(stop) = walk.stop {
        return stop;
    }
    if walk.frontier.is_empty() {
        // The genuine end of history, and the only value that justifies hiding "load more".
        return LogStop::Exhausted;
    }
    // Rows are still queued. Which of the two page-level budgets stopped the loop does not matter
    // here — under a merged scope it may have been *another* repository that filled the page —
    // and either way there is more in this one, which is exactly what `Page` says. The loop cannot
    // exit with a non-empty frontier and no budget spent, because every frontier entry has a heap
    // entry and an empty heap is the only other way out.
    LogStop::Page
}

fn build_resume(
    walks: &[Walk<'_>],
    identity: &str,
    lanes: Option<LaneResume>,
    cursor_lost: bool,
) -> Option<LogResume> {
    // `None` is the only honest way to say "there is no more", and it is deliberately not
    // inferable from `stop`: a caller that re-derived the rule gets `CursorLost` wrong and
    // produces an infinite "load more" returning the same zero rows for ever.
    if cursor_lost || walks.iter().all(|w| w.frontier.is_empty()) {
        return None;
    }
    let mut repos = Vec::with_capacity(walks.len());
    for walk in walks {
        // A commit can only be re-reached through a frontier entry, so the oids that have to
        // survive are the ones no newer than the newest thing still queued. Under a sane clock
        // that is the tie group at the boundary — a handful.
        let ceiling = walk
            .frontier
            .values()
            .map(|p| p.key)
            .max()
            .unwrap_or(i64::MIN);
        let mut hidden: Vec<String> = walk
            .seen_keys
            .iter()
            .filter(|(_, key)| *key <= ceiling)
            .map(|(oid, _)| oid.to_string())
            .collect();
        if hidden.len() > HIDDEN_CAP {
            hidden.drain(..hidden.len() - HIDDEN_CAP);
        }
        repos.push(RepoResume {
            repo: walk.info.id,
            frontier: walk
                .frontier
                .iter()
                .map(|(oid, pending)| FrontierRef {
                    oid: oid.to_string(),
                    key: pending.key,
                    paths: pending.paths.iter().map(|p| p.to_string()).collect(),
                })
                .collect(),
            hidden,
            watermark: walk.watermark,
            recent: walk.recent.iter().map(Oid::to_string).collect(),
            scanned: walk.scanned_total,
            done: walk.frontier.is_empty(),
        });
    }
    Some(LogResume {
        version: TOKEN_VERSION,
        key: identity.to_string(),
        repos,
        lanes,
    })
}

/// Record an oid as visited, keeping the tail of the sequence for the token.
///
/// `recent` is the clock-skew guard across a page boundary. A descendant carrying an *older* stamp
/// than its ancestor breaks the "keys are non-increasing" argument by exactly the width of the
/// skew, and the ring is what stops the overlap becoming a visibly duplicated row. It is a bound,
/// not a proof: perfect dedupe means carrying every emitted oid, and that is out of scope by
/// choice rather than by oversight.
fn remember(walk: &mut Walk<'_>, oid: Oid, key: i64) {
    walk.recent.push_back(oid);
    if walk.recent.len() > RECENT_CAP {
        walk.recent.pop_front();
    }
    walk.seen_keys.push_back((oid, key));
    if walk.seen_keys.len() > HIDDEN_CAP {
        walk.seen_keys.pop_front();
    }
}

/// The fingerprint a resume token carries.
///
/// Every field of the query **except** the cursor and the two budgets: those can change between
/// pages (a user widens the budget and keeps scrolling) without invalidating the frontier.
/// Everything else changes what the frontier *means*, and handing a token from one query to
/// another produces a page that is neither.
///
/// Length-prefixed rather than joined by a separator, so no combination of a path and an author
/// can collide with a different combination of the same characters.
fn query_identity(query: &LogQuery) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    match &query.scope {
        LogScope::One { repo } => {
            field(b"one");
            field(repo.to_string().as_bytes());
        }
        LogScope::Merged { repos } => {
            field(b"merged");
            for repo in repos {
                field(repo.to_string().as_bytes());
            }
        }
    }
    match &query.refs {
        LogRefs::Head => field(b"head"),
        LogRefs::Branch { name } => {
            field(b"branch");
            field(name.as_bytes());
        }
        LogRefs::All => field(b"all"),
        LogRefs::Rev { spec } => {
            field(b"rev");
            field(spec.as_bytes());
        }
    }
    field(query.path.as_deref().unwrap_or("").as_bytes());
    field(&[u8::from(query.follow)]);
    field(&[u8::from(query.simplify == Simplify::Full)]);
    field(&[u8::from(query.first_parent)]);
    field(query.author.as_deref().unwrap_or("").as_bytes());
    field(query.text.as_deref().unwrap_or("").as_bytes());
    field(&query.graph_lanes.to_le_bytes());
    hasher.finalize().to_hex()[..32].to_string()
}

// --- odds and ends -------------------------------------------------------------------------------------------

fn clamp_limit(limit: u32) -> u32 {
    if limit == 0 {
        LIMIT_DEFAULT
    } else {
        limit.min(LIMIT_MAX)
    }
}

fn clamp_scan(scan: u32) -> u32 {
    if scan == 0 {
        SCAN_DEFAULT
    } else {
        scan.min(SCAN_MAX)
    }
}

fn scope_repos<'a>(repos: &'a [RepoInfo], scope: &LogScope) -> Result<Vec<&'a RepoInfo>> {
    let ids = match scope {
        LogScope::One { repo } => vec![*repo],
        LogScope::Merged { repos } => repos.clone(),
    };
    ids.into_iter()
        .map(|id| {
            repos
                .iter()
                .find(|info| info.id == id)
                .ok_or(GitError::NoSuchRepo { repo: id })
        })
        .collect()
}

/// The graft points of a shallow clone, read once.
///
/// `commondir` and not `path`: a linked worktree's own gitdir holds `HEAD` and `index`, while
/// `shallow` lives with the objects it describes, in the common directory — the same split
/// `repo::watch_dirs` exists for.
fn shallow_set(repo: &Repository) -> HashSet<Oid> {
    let file = repo.commondir().join("shallow");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return HashSet::new();
    };
    text.lines()
        .filter_map(|line| Oid::from_str(line.trim()).ok())
        .collect()
}
