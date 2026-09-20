//! The properties card's Git block, against real repositories. (M70)
//!
//! `cide_git::properties` is two bounded walks over `cide_git::log`, and almost everything it
//! could get wrong is a wrong *number* rather than a failure: a button that is missing on the
//! one file that needs it, a "tracked since" date that is really the date of a rename, an
//! "untracked" verdict on a file whose history was merely too long to read. None of those throws
//! and none of them looks wrong on screen, so each has a test here.
//!
//! Every commit gets its own second, for `tests/log.rs`'s reason: `git log` breaks a
//! committer-time tie by insertion order and our heap breaks it by oid, so a fixture that
//! commits several times inside one wall-clock second would be testing the tie-break rather than
//! the walk. `commit_all_at` is how that is said.

mod support;

use cide_git::properties::{RECENT_LIMIT, path_summary};
use cide_git::repo as repo_mod;
use cide_ipc::git::{RepoInfo, RepoPath};
use support::TempRepo;

fn info(repo: &TempRepo) -> RepoInfo {
    repo_mod::discover(std::slice::from_ref(&repo.root))
        .into_iter()
        .next()
        .expect("the temp repo is discoverable")
}

/// A summary for `path` in `repo`, with the repository discovered the way the command does.
fn summary(repo: &TempRepo, path: &str) -> cide_ipc::properties::FilePropertiesGit {
    let info = info(repo);
    let located = RepoPath {
        repo: info.id,
        path: path.to_string(),
    };
    path_summary(std::slice::from_ref(&info), &located).expect("summary")
}

/// `n` commits to `path`, one second apart, oldest first. Returns the messages in that order.
fn history(repo: &TempRepo, path: &str, n: u32) -> Vec<String> {
    let mut messages = Vec::new();
    for i in 0..n {
        repo.write(path, format!("v{i}\n").as_bytes());
        let message = format!("c{i}");
        repo.commit_all_at(&message, 1_700_000_000 + i64::from(i));
        messages.push(message);
    }
    messages
}

#[test]
fn the_ends_of_the_list_are_the_newest_and_oldest_commits() {
    let repo = TempRepo::new("props-ends");
    let messages = history(&repo, "a.txt", 3);

    let s = summary(&repo, "a.txt");

    assert_eq!(s.recent.len(), 3);
    // Newest first, like every other list of commits in cide.
    assert_eq!(s.recent[0].summary, messages[2]);
    assert_eq!(s.last_commit.expect("last").summary, messages[2]);
    assert_eq!(s.first_commit.expect("first").summary, messages[0]);
    assert!(!s.more);
    assert!(!s.first_truncated);
}

#[test]
fn a_path_with_no_history_is_empty_rather_than_an_error() {
    // An untracked file is an ordinary thing to right-click — `FileTree.tsx` makes the same
    // argument for not disabling *Show File History*. The card says so; it does not fail.
    let repo = TempRepo::new("props-untracked");
    history(&repo, "a.txt", 1);
    repo.write("never-committed.txt", b"hello\n");

    let s = summary(&repo, "never-committed.txt");

    assert!(s.recent.is_empty());
    assert!(s.first_commit.is_none());
    assert!(s.last_commit.is_none());
    assert!(!s.more);
    // The flag is the whole reason the card may word this as "not committed": nothing was
    // truncated, so the absence is a fact about the file and not about our budget.
    assert!(!s.first_truncated);
}

#[test]
fn more_is_read_from_the_extra_row_and_not_from_the_length() {
    // The off-by-one this pins: a `recent.len() == RECENT_LIMIT` test claims there is more
    // history and draws *Open full history* onto a file whose entire history is already on
    // screen — and, worse, is right often enough that nobody notices.
    let repo = TempRepo::new("props-more");
    history(&repo, "a.txt", RECENT_LIMIT);

    let s = summary(&repo, "a.txt");
    assert_eq!(s.recent.len(), RECENT_LIMIT as usize);
    assert!(!s.more, "exactly the limit is not more than the limit");

    repo.write("a.txt", b"one more\n");
    repo.commit_all_at("extra", 1_700_001_000);

    let s = summary(&repo, "a.txt");
    assert_eq!(
        s.recent.len(),
        RECENT_LIMIT as usize,
        "the list stays capped"
    );
    assert!(s.more);
}

#[test]
fn the_list_holds_only_commits_that_touched_the_path() {
    let repo = TempRepo::new("props-filter");
    repo.write("a.txt", b"a\n");
    repo.commit_all_at("touches a", 1_700_000_000);
    repo.write("b.txt", b"b\n");
    repo.commit_all_at("touches b", 1_700_000_001);

    let s = summary(&repo, "a.txt");

    assert_eq!(s.recent.len(), 1);
    assert_eq!(s.recent[0].summary, "touches a");
    assert_eq!(s.rel_path, "a.txt");
}

#[test]
fn tracked_since_follows_a_rename() {
    // Most of the value of the row. Without `follow`, a file moved last week reports itself as
    // created last week, which is both wrong and the opposite of what the reader is asking.
    let repo = TempRepo::new("props-rename");
    repo.write("old.txt", b"contents\n");
    repo.commit_all_at("created as old.txt", 1_700_000_000);
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.commit_all_at("renamed to new.txt", 1_700_000_060);

    let s = summary(&repo, "new.txt");

    assert_eq!(
        s.first_commit.expect("first").summary,
        "created as old.txt",
        "the rename was not followed, so the file looks newer than it is"
    );
    assert_eq!(s.recent.len(), 2);
}

#[test]
fn a_directory_path_has_a_history_too() {
    // The card is offered on folders, so this has to answer. git logs a directory prefix
    // happily; the tool window's history *tabs* are per-file, which is a different limitation
    // and the one `FileTree.tsx`'s refusal sentence is about.
    let repo = TempRepo::new("props-dir");
    repo.write("src/a.txt", b"a\n");
    repo.commit_all_at("in src", 1_700_000_000);
    repo.write("other.txt", b"o\n");
    repo.commit_all_at("outside src", 1_700_000_001);

    let s = summary(&repo, "src");

    assert_eq!(s.recent.len(), 1);
    assert_eq!(s.recent[0].summary, "in src");
}

#[test]
fn the_rows_carry_what_the_card_prints() {
    // The card draws an author, a date and a short oid off these rows. A field that came back
    // empty would render as a blank column rather than as an error.
    let repo = TempRepo::new("props-fields");
    history(&repo, "a.txt", 1);

    let s = summary(&repo, "a.txt");
    let row = &s.recent[0];

    assert!(!row.author.is_empty());
    assert!(!row.short_oid.is_empty());
    assert_eq!(row.oid.len(), 40);
    assert!(row.authored > 0);
    assert_eq!(row.summary, "c0");
}

#[test]
fn last_commit_is_the_head_of_the_list_and_not_a_second_walk() {
    // One producer for the summary line and the list. Two walks could disagree — the list is
    // path-filtered and simplified, and a "last commit" derived any other way would name a
    // commit the list below it does not contain.
    let repo = TempRepo::new("props-head");
    history(&repo, "a.txt", 4);

    let s = summary(&repo, "a.txt");

    assert_eq!(
        s.last_commit.as_ref().expect("last").oid,
        s.recent[0].oid,
        "the summary line names a commit the list does not start with"
    );
}
