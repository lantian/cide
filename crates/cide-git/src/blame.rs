//! Per-line attribution: who last touched each line, and where the line came from before that.
//!
//! # The two findings that shape this module, both read out of the vendored source
//!
//! `libgit2-sys 0.18.7+1.9.6` is libgit2 1.9.6, and this is what it says about blame.
//!
//! **1. git2's four copy-tracking setters are inert.**
//! `libgit2/include/git2/blame.h:36-64` declares `GIT_BLAME_TRACK_COPIES_SAME_FILE` (`:40`),
//! `GIT_BLAME_TRACK_COPIES_SAME_COMMIT_MOVES` (`:48`), `GIT_BLAME_TRACK_COPIES_SAME_COMMIT_COPIES`
//! (`:56`) and `GIT_BLAME_TRACK_COPIES_ANY_COMMIT_COPIES` (`:64`), and every one of the four
//! carries the same sentence: *"This is not yet implemented and reserved for future use."*
//! git2-rs exposes all four as [`git2::BlameOptions::track_copies_same_file`] and friends, they
//! set a bit in a struct, and nothing in libgit2 ever reads that bit. Calling them costs nothing
//! and **does** nothing.
//!
//! That is the exact failure class `cide-core`'s dead `repoOpen` flag is memorialised for: an
//! option that looks set, reads as set, and changes no behaviour, so the feature appears to be
//! wired up and is silently the default answer for ever. So [`BlameFollow::MovedLines`],
//! [`BlameFollow::CopiesInCommit`] and [`BlameFollow::CopiesAnywhere`] — git's `-M`, `-C` and
//! `-C -C -C` — **shell out to the `git` binary**, and this module never touches those four
//! setters. If a later reader "tidies up" by adding them back because the enum variants exist,
//! the answer will not change and nothing will fail; that is why this paragraph is here and why
//! [`crate::blame`]'s test suite asserts the *observable* difference instead.
//!
//! **2. Whole-file rename following is already on, unconditionally, and is free.**
//! `libgit2/src/libgit2/blame_git.c:470` sets `findopts.flags = GIT_DIFF_FIND_RENAMES` and runs
//! `git_diff_find_similar` on the full tree-to-tree diff at *every* step of the walk — not
//! behind a flag, not behind an option, on every parent hop. So a file that was `git mv`'d keeps
//! its history through libgit2 alone, and that is what fills [`BlameHunk::path`] and therefore
//! [`BlameCommit::orig_path`]. Only *line-level* move and copy detection needs the binary.
//!
//! The two findings together are why [`BlameFile::follow`] reports what was **actually** run
//! rather than what was asked for, and why [`BlameFile::downgraded`] exists: an expensive mode
//! that quietly answered as a cheap one is a gutter that looks like the expensive one and is not.
//!
//! # The invariant everything else rests on
//!
//! [`check_runs`]. The runs must be ascending, gapless, and cover exactly `1..=lines`. It is
//! applied to **every** route — libgit2's and the binary's alike — rather than trusted from
//! either, because a run set with a hole paints a gutter that is silently one line off for
//! everything below the hole, **and every line still carries *a* label**, so nothing downstream
//! can notice: not the renderer, which indexes by line and finds one; not the tooltip, which
//! shows a real commit; not the user, who has no second opinion on screen. A wrong blame that
//! looks exactly like a right blame is the whole risk of this feature, and this function is the
//! only thing standing between the two.
//!
//! # Why the commit table costs nothing
//!
//! [`collect`] builds every [`BlameCommit`] out of the hunk itself — `final_commit_id`,
//! `final_signature`, `summary`, `path` and `is_boundary` are all on `git_blame_hunk` — and
//! therefore performs **not a single `find_commit`**. That is the entire reason a blame of a
//! five-thousand-line file is a cheap call: the alternative is one object lookup, one commit
//! parse and one signature parse per distinct commit, three hundred times, to produce data
//! libgit2 already handed us. The table is deduplicated by oid and the runs index into it, so
//! the same commit's name and address ride the wire once.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use cide_core::child_env::{FilterError, Filtered};
use cide_ipc::git::GitError;
use cide_ipc::history::{
    BlameCommit, BlameFile, BlameFollow, BlameParent, BlameRequest, BlameRun, BlameSource,
};
use git2::{Blame, BlameHunk, BlameOptions, Commit, ObjectType, Oid, Repository, Tree};

use crate::{Result, Wrap, diff, repo as repo_mod};

/// The largest file this will blame.
///
/// Blame cannot be cancelled — libgit2 exposes no hook inside `git_blame_file` and the binary
/// route's only lever is a kill — so the input is the bound. Two megabytes is roughly fifty
/// thousand lines of source, past which the walk is seconds and the gutter is unreadable
/// anyway.
pub const MAX_BLAME_BYTES: u64 = 2 * 1024 * 1024;

/// How long the `git blame -M`/`-C` shell-out is allowed to take before it is killed and the
/// libgit2 answer is used instead.
///
/// `-C -C -C` searches every file in every parent commit and git's own documentation warns it
/// can take minutes on a large repository. A blame that never returns is a pane that never
/// paints, and the fallback is a correct, cheaper answer that says so — so the deadline is
/// generous enough that the mode is usable and short enough that nobody watches a spinner.
const BINARY_DEADLINE: Duration = Duration::from_secs(20);

/// Hex characters in an abbreviated oid on this surface.
///
/// A fixed width and not libgit2's uniqueness rule (`Object::short_id`), because that rule
/// needs an object lookup per commit and the point of [`collect`] is that it performs none.
/// Eight is what [`cide_ipc::git::BranchRef::tip`] and [`cide_ipc::git::PulledCommit::short_oid`]
/// already use, so a short oid in the gutter reads the same as one in a pull toast.
const SHORT_OID: usize = 8;

/// Blame `path`, subject to [`MAX_BLAME_BYTES`].
///
/// `contents` is the **editor buffer** when the tab is dirty — see [`blame_with`] for what the
/// three sources mean and when each is chosen.
pub fn blame(
    root: &Path,
    path: &str,
    contents: Option<&[u8]>,
    request: &BlameRequest,
) -> Result<BlameFile> {
    blame_with(root, path, contents, request, MAX_BLAME_BYTES)
}

/// [`blame`] with the size cap as an argument.
///
/// The cap is a parameter for exactly one reason: a test has to be able to exercise the refusal.
/// Generating two megabytes of source to prove that [`GitError::FileTooLarge`] carries the right
/// numbers would make the suite slow and the assertion no stronger, and a test that instead
/// reached inside and skipped the check would be testing a different function from the one that
/// ships.
///
/// # The order of operations, and why it is this order
///
/// Nothing expensive happens before the cap. Open, the HEAD tree, the tree entry, the size — in
/// that sequence — so that a twelve-megabyte generated file is refused after four cheap lookups
/// and before anything reads a blob or walks a commit. Each step also has its own refusal, and
/// they are genuinely different questions: an unborn HEAD ([`GitError::Unborn`]) is a repository
/// with no history at all, a missing tree entry ([`GitError::NotTracked`]) is a file git has
/// never seen, and neither is "too large".
///
/// # Which version of the file is blamed
///
/// libgit2 blames the **committed blob**: `blame.c:411`'s `load_blob` looks the path up under
/// `newest_commit` (defaulting to `HEAD`, `blame.c:267-269`) and never reads the working tree.
/// So the working file and the editor buffer are reached the same way, through
/// [`git2::Blame::blame_buffer`], which re-maps a base blame onto modified content and gives
/// every line that differs libgit2's zero oid — which becomes [`BlameRun::commit`] `None` and
/// [`BlameFile::dirty`].
///
/// * `contents` given → [`BlameSource::Buffer`]. The tab is dirty; only the caller has these
///   bytes.
/// * else a working file that differs from the committed blob → [`BlameSource::Working`].
/// * else → [`BlameSource::Head`]: the file on disk is byte-identical to the commit, so the
///   re-map would be an xdiff of a file against itself for an answer already in hand.
/// * [`BlameRequest::newest`] set → [`BlameSource::Revision`], and the buffer is ignored: a
///   historical revision is frozen and today's unsaved edits are not part of it. (git refuses
///   the combination outright — `--contents` and a final commit name cannot both be given.)
pub fn blame_with(
    root: &Path,
    path: &str,
    contents: Option<&[u8]>,
    request: &BlameRequest,
    max_bytes: u64,
) -> Result<BlameFile> {
    let repo = repo_mod::open(root)?;

    // The commit the whole answer is relative to. With `newest` this is a historical revision;
    // without it, HEAD. `head_tree` is asked rather than `repo.head()` because an unborn HEAD is
    // a normal state everywhere else in this crate and libgit2 reports it as an error code.
    let (head_oid, tree) = match request.newest.as_deref() {
        Some(spec) => {
            let commit = resolve_commit(&repo, spec)?;
            let tree = commit.tree().wrap()?;
            (commit.id(), tree)
        }
        None => {
            let Some(tree) = diff::head_tree(&repo)? else {
                return Err(GitError::Unborn);
            };
            (repo.refname_to_id("HEAD").wrap()?, tree)
        }
    };

    let entry = tree
        .get_path(Path::new(path))
        .map_err(|_| GitError::NotTracked {
            path: path.to_string(),
        })?;
    // A directory resolves to a tree and a submodule to a commit, and blame means nothing for
    // either. `NotTracked` rather than a libgit2 error, because from the caller's side the
    // question — "is there a blame for this path" — has the same answer as for a path git has
    // never seen, and the gutter's empty state already says it.
    if entry.kind() != Some(ObjectType::Blob) {
        return Err(GitError::NotTracked {
            path: path.to_string(),
        });
    }

    let on_disk = root.join(path);
    // `metadata` and not a read: the size check exists to refuse before anything expensive, and
    // reading twelve megabytes to discover they are twelve megabytes defeats it.
    let disk_len = std::fs::metadata(&on_disk)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len());
    // Whatever is actually going to be blamed, and in the same order the source is chosen below —
    // the two must not disagree, or the cap measures one file and the walk reads another.
    let bytes_len = if request.newest.is_some() {
        // A historical revision ignores both the buffer and the file on disk, so neither of their
        // sizes is the one to refuse on.
        repo.find_blob(entry.id()).wrap()?.size() as u64
    } else if let Some(buffer) = contents {
        buffer.len() as u64
    } else if let Some(len) = disk_len {
        len
    } else {
        repo.find_blob(entry.id()).wrap()?.size() as u64
    };
    if bytes_len > max_bytes {
        return Err(GitError::FileTooLarge {
            path: path.to_string(),
            bytes: bytes_len,
            limit: max_bytes,
        });
    }

    let committed = repo.find_blob(entry.id()).wrap()?;
    let (source, bytes) = if request.newest.is_some() {
        (BlameSource::Revision, None)
    } else if let Some(buffer) = contents {
        (BlameSource::Buffer, Some(buffer.to_vec()))
    } else if disk_len.is_some() {
        let disk = std::fs::read(&on_disk).wrap()?;
        if disk == committed.content() {
            (BlameSource::Head, None)
        } else {
            (BlameSource::Working, Some(disk))
        }
    } else {
        (BlameSource::Head, None)
    };

    // Counted from the bytes rather than summed from the hunks, and that is the point: a total
    // derived from the same hunks it is checking would make [`check_runs`]'s coverage clause
    // vacuous. This is the independent second opinion.
    let lines = count_lines(bytes.as_deref().unwrap_or_else(|| committed.content()));

    let mut follow = BlameFollow::Renames;
    let mut downgraded = None;
    let (runs, commits) = if request.follow == BlameFollow::Renames {
        via_libgit2(&repo, path, bytes.as_deref(), request, head_oid, lines)?
    } else {
        match via_binary(root, path, bytes.as_deref(), request, head_oid, lines) {
            Ok(pair) => {
                follow = request.follow;
                pair
            }
            Err(reason) => {
                downgraded = Some(reason);
                via_libgit2(&repo, path, bytes.as_deref(), request, head_oid, lines)?
            }
        }
    };

    Ok(BlameFile {
        path: path.to_string(),
        head: head_oid.to_string(),
        lines,
        // Literally what the field says: *the blamed source differs from `head`*. Derived from
        // the bytes rather than from the runs, because the two disagree in a case that really
        // happens — a buffer whose only edit is a **deleted** line has no uncommitted line to
        // mark, so every run still carries a commit while the gutter is nonetheless not a
        // picture of HEAD. Derived from the bytes rather than from `source`, because a caller
        // that hands us a buffer identical to the commit is looking at a clean file and should
        // not be told otherwise.
        dirty: bytes
            .as_deref()
            .is_some_and(|blamed| blamed != committed.content()),
        runs,
        commits,
        source,
        follow,
        downgraded,
    })
}

// --- the libgit2 route --------------------------------------------------------------------

fn via_libgit2(
    repo: &Repository,
    path: &str,
    bytes: Option<&[u8]>,
    request: &BlameRequest,
    head: Oid,
    lines: u32,
) -> Result<(Vec<BlameRun>, Vec<BlameCommit>)> {
    let mut options = BlameOptions::new();
    // The three options that are actually implemented. `use_mailmap` is not cosmetic: without
    // it a repository with a `.mailmap` gets a gutter naming people `git blame` does not, and
    // the two sitting side by side is how a user concludes the gutter is broken.
    options.use_mailmap(true);
    options.first_parent(request.first_parent);
    options.ignore_whitespace(request.ignore_whitespace);
    // `newest_commit` is set unconditionally: it is where `head` came from, so passing it back
    // makes the answer independent of `HEAD` moving between the two reads.
    options.newest_commit(head);
    // The four `track_copies_*` setters are deliberately absent. See the module header — they
    // are documented in libgit2 as unimplemented, and setting them would claim a mode this
    // route cannot deliver.

    let base = repo
        .blame_file(Path::new(path), Some(&mut options))
        .wrap()?;

    // **A lifetime trap worth the comment.** `git2::Blame::blame_buffer` returns a
    // `Blame<'_>` borrowed from `&self` — the derived blame holds pointers into the base
    // blame's hunks and `git_blame_free` on the base invalidates them. So the collect happens
    // *inside* the same statement that produces the buffer blame, and there is deliberately no
    // `fn buffer_blame(&base) -> Blame` helper here: any such helper drops the base as it
    // returns and hands back a structure of dangling pointers, which the borrow checker only
    // catches when the base is a local of the helper — write it as a method-chain returning
    // from a function that owns both and it compiles.
    let (runs, commits) = match bytes {
        Some(buffer) => collect(&base.blame_buffer(buffer).wrap()?, path),
        None => collect(&base, path),
    };

    check_runs(&runs, lines)?;
    Ok((runs, commits))
}

/// Turn libgit2's hunks into runs and a deduplicated commit table.
///
/// Not a single `find_commit`; see the module header for why that is the whole point.
fn collect(blame: &Blame<'_>, path: &str) -> (Vec<BlameRun>, Vec<BlameCommit>) {
    let mut runs: Vec<BlameRun> = Vec::new();
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut seen: HashMap<Oid, usize> = HashMap::new();

    for hunk in blame.iter() {
        let lines = hunk.lines_in_hunk() as u32;
        if lines == 0 {
            continue;
        }
        let start = hunk.final_start_line() as u32;
        let oid = hunk.final_commit_id();

        let slot = if oid.is_zero() {
            // libgit2's marker for a line that is in the buffer and in no commit — see
            // `blame.c`'s `hunk_is_bufferblame`. It is not an error and not a missing lookup.
            None
        } else {
            let at = match seen.get(&oid) {
                Some(&at) => at,
                None => {
                    let at = commits.len();
                    commits.push(commit_from_hunk(&hunk, oid, path));
                    seen.insert(oid, at);
                    at
                }
            };
            // OR'd across the commit's hunks rather than taken from the first: boundary is a
            // property of how far the *walk* got, and a commit can be the boundary for one span
            // of lines and an ordinary attribution for another. Losing the flag would present a
            // shallow clone's floor as authorship.
            if hunk.is_boundary() {
                commits[at].boundary = true;
            }
            Some(at as u32)
        };

        // Adjacent runs of one commit are merged. libgit2 emits them routinely after a buffer
        // re-map — an edit in the middle of a span splits it into three hunks and the outer two
        // are re-joined here once the edit's own hunk is passed — and merging shrinks the
        // payload for free. Adjacency is checked, not assumed: merging across a gap would close
        // the hole that [`check_runs`] exists to find.
        match runs.last_mut() {
            Some(previous)
                if previous.commit == slot && previous.start + previous.lines == start =>
            {
                previous.lines += lines;
            }
            _ => runs.push(BlameRun {
                start,
                lines,
                commit: slot,
            }),
        }
    }

    (runs, commits)
}

fn commit_from_hunk(hunk: &BlameHunk<'_>, oid: Oid, path: &str) -> BlameCommit {
    let signature = hunk.final_signature();
    let (author, email, authored) = match &signature {
        // The `*_bytes` accessors, because the `&str` ones return `None` for a non-UTF-8 name
        // and a gutter with a blank author is worse than one with a replacement character.
        Some(signature) => (
            String::from_utf8_lossy(signature.name_bytes()).into_owned(),
            String::from_utf8_lossy(signature.email_bytes()).into_owned(),
            signature.when().seconds(),
        ),
        None => (String::new(), String::new(), 0),
    };
    BlameCommit {
        oid: oid.to_string(),
        short_oid: short(oid),
        summary: hunk
            .summary_bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default(),
        author,
        author_email: email,
        authored,
        // The path the file had at this commit. Populated by libgit2's unconditional
        // `find_similar` (see the module header), so this is the field that proves rename
        // following is on. `None` when it has not changed, so the frontend can test one thing.
        orig_path: hunk
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| p != path),
        boundary: hunk.is_boundary(),
    }
}

// --- the run invariant ----------------------------------------------------------------------

/// Refuse a run set that is not ascending, gapless, and exactly `1..=lines`.
///
/// Public so both routes and their tests call the same function. See the module header for why
/// this is the one check that cannot be skipped: every failure it catches produces a gutter that
/// is confidently, plausibly and undetectably wrong.
pub fn check_runs(runs: &[BlameRun], lines: u32) -> Result<()> {
    let mut expected = 1u32;
    for (index, run) in runs.iter().enumerate() {
        if run.lines == 0 {
            return Err(GitError::Git {
                detail: format!("blame run {index} covers no lines"),
            });
        }
        if run.start != expected {
            return Err(GitError::Git {
                detail: format!(
                    "blame run {index} starts at line {} where {expected} was expected",
                    run.start
                ),
            });
        }
        expected = expected
            .checked_add(run.lines)
            .ok_or_else(|| GitError::Git {
                detail: format!("blame run {index} overflows the line counter"),
            })?;
    }
    if expected != lines + 1 {
        return Err(GitError::Git {
            detail: format!(
                "blame runs cover {} lines of {lines}",
                expected.saturating_sub(1)
            ),
        });
    }
    Ok(())
}

// --- the `git` binary route -------------------------------------------------------------------

/// `git blame --porcelain` with `-M` or `-C`, for the modes libgit2 does not implement.
///
/// The error type is a **sentence**, not a [`GitError`]: every way this can fail — the binary is
/// missing, it refused, it ran past the deadline, its output did not parse — has the same
/// recovery, which is to answer with the libgit2 rename-follow blame and say so in
/// [`BlameFile::downgraded`]. Turning any of them into a refusal would trade a correct, cheaper
/// gutter for an error message.
fn via_binary(
    root: &Path,
    path: &str,
    bytes: Option<&[u8]>,
    request: &BlameRequest,
    head: Oid,
    lines: u32,
) -> std::result::Result<(Vec<BlameRun>, Vec<BlameCommit>), String> {
    let mut command = Command::new("git");
    // The two mandatory spawn passes are applied by `run_filter` below, at the chokepoint, and
    // deliberately not here. They used to be one line at this spot — `prepare_command` without
    // its `arm` partner, which is the half that had been missing since this route was written.
    //
    // Why they matter for `git` specifically: an AppImage's `AppRun` leaves `LD_LIBRARY_PATH`
    // and `PYTHONHOME` pointing inside the mounted image, and `git` dlopens the host's libcurl
    // and OpenSSL — so a `git` that inherits cide's bundled environment picks up eleven bundled
    // libraries ahead of the host's and fails in ways reported from three processes below
    // anything cide logs. `prepare_command` also appends `cide_core::toolchain::extra_dirs` to
    // `PATH`, which is what lets a GUI-launched cide find a `git` that is not in the desktop
    // session's `PATH` at all. See ADR 0007 and ADR 0008.
    command.current_dir(root);
    command.arg("blame").arg("--porcelain");
    // Plain `--porcelain` and **not** `--line-porcelain`: the latter repeats the whole commit
    // header for every single line, which for a five-thousand-line file is megabytes of
    // duplicated author and summary text to parse and throw away. Collapsing repeats is exactly
    // what makes this payload small, and `--porcelain` has already done it.
    match request.follow {
        // Reached only from `blame_with`'s non-`Renames` arm; kept exhaustive so a new variant
        // is a compile error here rather than a silent plain blame.
        BlameFollow::Renames => {}
        BlameFollow::MovedLines => {
            command.arg("-M");
        }
        BlameFollow::CopiesInCommit => {
            command.arg("-C");
        }
        BlameFollow::CopiesAnywhere => {
            command.args(["-C", "-C", "-C"]);
        }
    }
    if request.ignore_whitespace {
        command.arg("-w");
    }
    if request.first_parent {
        command.arg("--first-parent");
    }
    // `--contents -` and a final commit name are mutually exclusive in git, which is fine
    // because they are mutually exclusive here too: `blame_with` produces bytes only when it is
    // blaming the buffer or the working file, and a revision blame has neither.
    match bytes {
        Some(_) => {
            command.args(["--contents", "-"]);
        }
        None => {
            command.arg(head.to_string());
        }
    }
    // `--` before the path, always: a file called `-M` or one that looks like a revision is a
    // file, and git cannot know that without the separator.
    command.arg("--").arg(path);

    let started = Instant::now();
    // The three-thread runner lives in `cide_core::child_env` since M26, because Reformat code
    // needs the identical thing to feed a dirty buffer to a formatter. The error is tagged
    // rather than prose precisely so this call site keeps saying "the gutter follows whole-file
    // renames only", which means nothing to a formatter.
    let Filtered { ok, stdout, stderr } = cide_core::child_env::run_filter(
        command,
        bytes,
        BINARY_DEADLINE,
    )
    .map_err(|error| match error {
        FilterError::Spawn(error) => format!("`git` could not be started ({error})"),
        FilterError::Timeout => format!(
            "`git blame` did not finish within {}s, so the gutter follows whole-file renames only",
            BINARY_DEADLINE.as_secs()
        ),
        FilterError::Unreadable => "`git blame`'s output could not be read".to_string(),
        FilterError::Wait(error) => {
            format!("`git blame` could not be waited for ({error})")
        }
    })?;
    if !ok {
        let message = stderr.trim();
        return Err(format!(
            "`git blame` refused this mode, so the gutter follows whole-file renames only{}",
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        ));
    }

    let (runs, commits, counted) = parse_porcelain(&stdout, path)?;
    if counted != lines {
        return Err(format!(
            "`git blame` reported {counted} lines where the file has {lines}, so the gutter \
             follows whole-file renames only"
        ));
    }
    // The invariant applies to this route exactly as it applies to libgit2's; the difference is
    // only in what a violation means. Here it means our porcelain parser is wrong, and the
    // recoverable answer — the libgit2 blame, which is itself checked — is better for the user
    // than an error, so it degrades with a sentence instead of refusing. The tests assert
    // totality on this route directly, so the bug is still loud where it can be fixed.
    if let Err(error) = check_runs(&runs, lines) {
        return Err(format!(
            "`git blame`'s output did not cover the file ({error}), so the gutter follows \
             whole-file renames only"
        ));
    }
    tracing::debug!(
        path,
        follow = ?request.follow,
        millis = started.elapsed().as_millis(),
        "blamed via the git binary"
    );
    Ok((runs, commits))
}

/// Everything the porcelain format says about one commit, accumulated until its content line.
#[derive(Default)]
struct Detail {
    author: String,
    email: String,
    authored: i64,
    summary: String,
    filename: Option<String>,
    boundary: bool,
}

/// Parse `git blame --porcelain`.
///
/// The format is one header line per *file* line — `<oid> <orig-lineno> <final-lineno>`, with a
/// group size appended on the first line of each group — optionally followed by the commit's
/// details, and terminated by the content line, which is the only line that starts with a tab.
/// **The details appear only the first time an oid is seen**, which is the whole reason
/// `--porcelain` is used instead of `--line-porcelain`, and it is why they are accumulated into
/// a table keyed by oid rather than read per group.
fn parse_porcelain(
    out: &[u8],
    path: &str,
) -> std::result::Result<(Vec<BlameRun>, Vec<BlameCommit>, u32), String> {
    let mut runs: Vec<BlameRun> = Vec::new();
    let mut commits: Vec<BlameCommit> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut lines = 0u32;

    let mut header: Option<(String, u32)> = None;
    let mut detail = Detail::default();

    for raw in out.split(|&byte| byte == b'\n') {
        if raw.first() == Some(&b'\t') {
            let (oid, reported) = header
                .take()
                .ok_or("a content line arrived before any header")?;
            lines += 1;
            // The strongest check the format allows, and it is cheap: git numbers every line and
            // we count them independently, so a group boundary misparsed by one is caught here
            // rather than becoming a silent one-line shift in the gutter.
            if reported != lines {
                return Err(format!(
                    "line {reported} arrived where {lines} was expected"
                ));
            }

            let slot = if oid.bytes().all(|byte| byte == b'0') {
                // git's zero oid: a line that is in the supplied contents or in the working tree
                // and in no commit. `author` reads `Not Committed Yet` or
                // `External file (--contents)`; neither belongs in the commit table.
                None
            } else {
                let at = match seen.get(&oid) {
                    Some(&at) => at,
                    None => {
                        let at = commits.len();
                        commits.push(BlameCommit {
                            short_oid: oid.chars().take(SHORT_OID).collect(),
                            oid: oid.clone(),
                            summary: std::mem::take(&mut detail.summary),
                            author: std::mem::take(&mut detail.author),
                            author_email: std::mem::take(&mut detail.email),
                            authored: detail.authored,
                            // Under `-C` one commit can appear with several filenames — that is
                            // what a copy *is*. The first is kept, matching the libgit2 route,
                            // where the hunk's `orig_path` is likewise per-commit on the wire.
                            orig_path: detail.filename.take().filter(|name| name != path),
                            boundary: false,
                        });
                        seen.insert(oid.clone(), at);
                        at
                    }
                };
                if detail.boundary {
                    commits[at].boundary = true;
                }
                Some(at as u32)
            };
            detail = Detail::default();

            match runs.last_mut() {
                Some(previous) if previous.commit == slot => previous.lines += 1,
                _ => runs.push(BlameRun {
                    start: lines,
                    lines: 1,
                    commit: slot,
                }),
            }
            continue;
        }

        if header.is_none() {
            if raw.is_empty() {
                // The empty tail after the final newline, and nothing else: any other blank line
                // would have been consumed as a key line above.
                continue;
            }
            let text = String::from_utf8_lossy(raw);
            let mut parts = text.split(' ');
            let oid = parts.next().unwrap_or_default();
            if oid.len() != 40 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("expected a blame header, got {text:?}"));
            }
            let _orig = parts.next();
            let reported: u32 = parts
                .next()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| format!("blame header without a line number: {text:?}"))?;
            header = Some((oid.to_string(), reported));
            continue;
        }

        let text = String::from_utf8_lossy(raw);
        let (key, value) = text.split_once(' ').unwrap_or((text.as_ref(), ""));
        match key {
            "author" => detail.author = value.to_string(),
            // Always angle-bracketed in the porcelain format; trimmed so the address matches
            // what libgit2's `Signature::email` gives on the other route.
            "author-mail" => {
                detail.email = value
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            }
            "author-time" => detail.authored = value.parse().unwrap_or_default(),
            "summary" => detail.summary = value.to_string(),
            "filename" => detail.filename = Some(value.to_string()),
            "boundary" => detail.boundary = true,
            // `committer*`, `previous`, `author-tz` and anything a later git adds. Ignored by
            // name rather than by position, so a new field does not shift the parse.
            _ => {}
        }
    }

    if header.is_some() {
        return Err("`git blame` ended in the middle of a group".to_string());
    }
    Ok((runs, commits, lines))
}

// --- the parent hop ---------------------------------------------------------------------------

/// The parent of `rev` **as it touched** `path`, following the rename at that boundary.
///
/// `None` at a root commit and at the commit that introduced the file. Both are the end of the
/// walk rather than a failure: the gutter's *Annotate previous revision* item is simply not
/// offered, and an error there would turn "this is where the file begins" into a red toast.
///
/// # Why this is not `rev^`
///
/// Two separate ways `rev^` is wrong, and each of them fails silently.
///
/// * `rev^` is the **first** parent. A merge that took the file from its second parent has a
///   first parent that never contained it, so blaming there answers about a file that is not
///   there — and libgit2 will happily return an empty blame rather than an error, so the pane
///   goes blank with nothing to explain it.
/// * `rev^` names the path as it is spelled **now**. Across the rename this gesture is most
///   often used to cross, the parent spells it differently, so the follow-up blame asks for a
///   path that does not exist at that revision — again, empty and quiet.
///
/// So the parents are searched in order for the first that contains the path, and the path is
/// mapped through that boundary's rename detection. Rename detection is run on the **whole**
/// tree-to-tree diff and not a pathspec-limited one: a pathspec hands `find_similar` one side of
/// the rename and it reports `Added`, which is the trap `cide-app`'s `git_diff_file` already
/// documents.
pub fn parent_of(root: &Path, path: &str, rev: &str) -> Result<Option<BlameParent>> {
    let repo = repo_mod::open(root)?;
    let commit = resolve_commit(&repo, rev)?;
    if commit.parent_count() == 0 {
        return Ok(None);
    }
    let tree = commit.tree().wrap()?;

    for parent in commit.parents() {
        let parent_tree = parent.tree().wrap()?;
        let Some(there) = path_in_parent(&repo, &parent_tree, &tree, path)? else {
            continue;
        };
        return Ok(Some(BlameParent {
            rev: parent.id().to_string(),
            short_rev: short(parent.id()),
            path: there,
            summary: summary_of(&parent),
        }));
    }
    Ok(None)
}

/// What `path` is called in `parent_tree`, or `None` if it is not there at all.
fn path_in_parent(
    repo: &Repository,
    parent_tree: &Tree<'_>,
    tree: &Tree<'_>,
    path: &str,
) -> Result<Option<String>> {
    // The cheap answer first: the overwhelmingly common case is that the parent spells it the
    // same way, and that costs one tree lookup instead of a full tree diff plus a similarity
    // pass over every added and deleted blob in the commit.
    if parent_tree.get_path(Path::new(path)).is_ok() {
        return Ok(Some(path.to_string()));
    }

    let mut options = git2::DiffOptions::new();
    options.include_typechange(true);
    let mut delta_diff = repo
        .diff_tree_to_tree(Some(parent_tree), Some(tree), Some(&mut options))
        .wrap()?;
    let mut find = git2::DiffFindOptions::new();
    // Renames only, copies off — the same `GIT_DIFF_FIND_RENAMES` libgit2's own blame walk uses
    // at `blame_git.c:470`. Turning copies on would let a file that merely *resembles* an
    // unmodified one be reported as its origin, which would send the hop to a path the line was
    // never in.
    find.renames(true);
    delta_diff.find_similar(Some(&mut find)).wrap()?;

    for delta in delta_diff.deltas() {
        let new_path = delta
            .new_file()
            .path_bytes()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        if new_path.as_deref() != Some(path) {
            continue;
        }
        // `Added` means this parent genuinely did not have it under any name; every other status
        // that reaches here came out of `find_similar` with an old side, which is the pre-rename
        // spelling this whole function exists to recover.
        if delta.status() == git2::Delta::Added {
            return Ok(None);
        }
        if let Some(old) = delta.old_file().path_bytes() {
            return Ok(Some(String::from_utf8_lossy(old).into_owned()));
        }
    }
    Ok(None)
}

// --- shared helpers ---------------------------------------------------------------------------

fn resolve_commit<'repo>(repo: &'repo Repository, rev: &str) -> Result<Commit<'repo>> {
    let object = repo
        .revparse_single(rev)
        .map_err(|error| match error.code() {
            git2::ErrorCode::Ambiguous => GitError::AmbiguousRev {
                spec: rev.to_string(),
            },
            _ => GitError::NoSuchRevision {
                rev: rev.to_string(),
            },
        })?;
    let kind = object.kind();
    object.peel_to_commit().map_err(|_| GitError::NotACommit {
        spec: rev.to_string(),
        // git's own word for what it found, so the sentence can say *`v1.2.0^{tree}` is a tree*.
        kind: kind.map(|kind| kind.str().to_string()).unwrap_or_default(),
    })
}

fn summary_of(commit: &Commit<'_>) -> String {
    commit
        .summary_bytes()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_default()
}

fn short(oid: Oid) -> String {
    oid.to_string().chars().take(SHORT_OID).collect()
}

/// Lines the way libgit2 counts them, so [`check_runs`] compares like with like.
///
/// `blame.c:331`'s `index_blob_lines`: every `\n` ends a line, and a final byte that is not `\n`
/// ends one more — the incomplete last line, which `git blame` also shows. An empty file has no
/// lines at all, which is why the runs may legitimately be empty.
fn count_lines(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let breaks = bytes.iter().filter(|&&byte| byte == b'\n').count();
    let incomplete = usize::from(bytes[bytes.len() - 1] != b'\n');
    (breaks + incomplete) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(start: u32, lines: u32, commit: Option<u32>) -> BlameRun {
        BlameRun {
            start,
            lines,
            commit,
        }
    }

    #[test]
    fn a_total_run_set_is_accepted() {
        let runs = [run(1, 3, Some(0)), run(4, 2, None), run(6, 1, Some(1))];
        assert!(check_runs(&runs, 6).is_ok());
    }

    #[test]
    fn an_empty_file_has_no_runs() {
        assert!(check_runs(&[], 0).is_ok());
    }

    /// The hole is the failure the whole feature rests on not having; it must be *rejected*,
    /// never quietly closed by shifting the following runs down.
    #[test]
    fn a_hole_is_refused() {
        let runs = [run(1, 2, Some(0)), run(4, 3, Some(1))];
        let error = check_runs(&runs, 6).expect_err("a gap must not be accepted");
        assert!(
            format!("{error:?}").contains("starts at line 4"),
            "{error:?}"
        );
    }

    #[test]
    fn a_short_cover_is_refused() {
        let runs = [run(1, 2, Some(0))];
        assert!(check_runs(&runs, 6).is_err());
    }

    #[test]
    fn an_overshoot_is_refused() {
        let runs = [run(1, 9, Some(0))];
        assert!(check_runs(&runs, 6).is_err());
    }

    #[test]
    fn an_empty_run_is_refused() {
        let runs = [run(1, 0, Some(0)), run(1, 6, Some(1))];
        assert!(check_runs(&runs, 6).is_err());
    }

    #[test]
    fn lines_are_counted_the_way_libgit2_counts_them() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a"), 1);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"\n"), 1);
    }
}
