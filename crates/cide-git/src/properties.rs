//! The git half of the properties card: what this repository remembers about one path. (M70)
//!
//! Three facts, from two bounded walks: the newest commit that touched the path, the oldest one
//! (the card's *tracked since*), and the handful in between that the card lists.
//!
//! # Why this is not `git_log`
//!
//! The obvious implementation is to let the webview call the paging command it already has. It
//! is wrong for a reason that has nothing to do with the rows.
//!
//! `git_log` keys its cancellation flag by `(project, tab: ToolTabId)` so that a superseded walk
//! stops within one commit instead of scanning to its budget for a page nobody will draw. A
//! modal is not a tool tab and has no id of its own. Handing it the Log tab's would cancel that
//! tab's in-flight page every time somebody opened a properties card — the walk would come back
//! `cancelled: true`, which is explicitly *not an error*, so the Log tab would simply go quiet
//! with no failure anywhere. Minting a sentinel id is the `DOCKER_TAB` hazard, and that one is
//! already written up as a thing which must never reach a command taking a uuid.
//!
//! So the walk happens here, sized small enough that there is nothing worth cancelling, and the
//! card's answer costs the Log tab nothing.
//!
//! # One producer, and it is [`log`](crate::log::log)
//!
//! Nothing here re-implements a revision walk. Both halves build a [`LogQuery`] and hand it to
//! the same function the Log tab uses, so a change to how cide follows renames or simplifies
//! merges reaches this card without anybody remembering it exists — `cide_git::push::preview`'s
//! rule, which exists because a preview that derived its own refspec drew a dialog that was
//! internally consistent about a different push.

use cide_ipc::git::{RepoInfo, RepoPath};
use cide_ipc::history::{CommitRow, LogCursor, LogQuery, LogRefs, LogScope, Simplify};
use cide_ipc::properties::FilePropertiesGit;

use crate::Result;
use crate::log;

/// How many commits the card lists. One screenful; the rest is the *Open full history* button,
/// which opens the tool tab that exists for exactly that.
pub const RECENT_LIMIT: u32 = 10;

/// Commits the *recent* walk may examine. Deliberately far below `log::SCAN_DEFAULT`.
///
/// This walk runs while somebody is looking at a card that is already on screen, and its rows
/// are a convenience with a full-history escape hatch beside them. A file touched once in a
/// forty-thousand-commit repository is the case that matters: with no budget the walk reads the
/// whole history to fill a list of one, on a blocking thread, for a panel the user is about to
/// close.
pub const RECENT_SCAN: u32 = 5_000;

/// Commits the *tracked since* walk may examine.
///
/// Larger, because this one cannot stop early by its nature — the oldest commit is at the far
/// end — and because the answer is worth more: "in this repository since April 2025" is the fact
/// people open a properties card for. Still bounded, and when it runs out the card says *more
/// than N commits of history* rather than inventing a date.
pub const FIRST_SCAN: u32 = 50_000;

/// Everything the card's Git block shows, or `None` when the path is in no repository.
///
/// `None` is not a failure: a file under `/tmp`, or in a project that was never `git init`ed, is
/// an ordinary thing to ask about, and the card draws no Git block at all rather than an empty
/// one making claims about a file git has never heard of.
pub fn path_summary(repos: &[RepoInfo], located: &RepoPath) -> Result<FilePropertiesGit> {
    let scope = LogScope::One { repo: located.repo };

    // Newest first, and one row more than the card shows. `more` is read from that extra row and
    // never from `recent.len() == RECENT_LIMIT`, which cannot tell a history of exactly ten from
    // a history of eleven — so the button that means "there is more to see" would be missing on
    // precisely the file that has ten commits.
    let recent_page = log::log(
        repos,
        &query(&scope, &located.path, RECENT_LIMIT + 1, RECENT_SCAN),
    )?;
    let mut recent = recent_page.commits;
    let more = recent.len() > RECENT_LIMIT as usize;
    recent.truncate(RECENT_LIMIT as usize);

    let last_commit = recent.first().cloned();
    let (first_commit, first_truncated) = oldest(repos, &scope, &located.path)?;

    Ok(FilePropertiesGit {
        repo: located.repo,
        rel_path: located.path.clone(),
        first_commit,
        first_truncated,
        last_commit,
        recent,
        more,
    })
}

/// The oldest commit touching `path`, and whether the walk gave up before finding it.
///
/// There is no "walk from the root" in libgit2's revwalk that would make this cheap — the graph
/// is only traversable from the tips — so this is the same walk with a large limit, keeping the
/// last row. The budget is what keeps it bounded, and the flag is what keeps it honest.
///
/// The distinction the flag carries is not cosmetic: `None` with `first_truncated: false` means
/// **this path has never been committed**, and `None` with `true` means **there is more history
/// than we were willing to read**. A card that collapsed them would tell somebody their file is
/// untracked because the repository is large.
fn oldest(repos: &[RepoInfo], scope: &LogScope, path: &str) -> Result<(Option<CommitRow>, bool)> {
    // `u32::MAX` as the row limit, with `FIRST_SCAN` as the real bound. The scan budget is the
    // one that matters — it counts commits *examined*, which for a path filter is the expensive
    // number — and capping the rows as well would stop the walk early on a file with a long
    // history and report a "first commit" that is merely the oldest of the first N.
    let page = log::log(repos, &query(scope, path, u32::MAX, FIRST_SCAN))?;

    // A frontier left over means the walk stopped before the beginning of history. `resume` is
    // the only honest reading of that — `stop` can say `Budget` or `Page` and still leave one,
    // and `CommitPage::resume`'s own doc comment says a frontend re-deriving the rule gets it
    // wrong. So it is read here, from the page, and not inferred.
    let truncated = page.resume.is_some();
    Ok((page.commits.last().cloned(), truncated))
}

/// One path-filtered query. The single place this feature spells a [`LogQuery`].
fn query(scope: &LogScope, path: &str, limit: u32, scan_limit: u32) -> LogQuery {
    LogQuery {
        scope: scope.clone(),
        refs: LogRefs::Head,
        cursor: LogCursor::Newest,
        path: Some(path.to_string()),
        // Renames are followed, which is most of the value of a *tracked since* row: a file that
        // was moved in 2024 was not created in 2024, and the card would otherwise say it was.
        // `CommitPage::followed` reports whether it actually applied; it always does here,
        // because `follow` is only ignored without a single path filter and there is one.
        follow: true,
        simplify: Simplify::Default,
        first_parent: false,
        author: None,
        text: None,
        limit,
        scan_limit,
        // No graph. `LogQuery::graph` documents that `false` skips the lane computation
        // *entirely* rather than computing and dropping it, and a card that draws no gutter must
        // not pay for one.
        graph: false,
        graph_lanes: 0,
    }
}
