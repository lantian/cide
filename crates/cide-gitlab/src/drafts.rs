//! Review comments that exist only in cide until the user publishes them.
//!
//! Local rather than GitLab's own draft notes, deliberately: an agent's findings must not be
//! visible to anyone — GitLab's "pending review" shows in the web UI and a stray "Submit review"
//! there would publish a machine's whole batch at once. The severity is local metadata for the
//! same reason: GitLab has no field for it, and writing it into the body would publish a label.
//!
//! A separate file from `gitlab.json` because that one holds tokens and is rewritten on every
//! account change; drafts are not secret, grow with every review, and must not make a corrupted
//! write of one lose the other.
use crate::Result;
use cide_ipc::gitlab::{
    GitLabDraft, GitLabDraftAuthor, GitLabDraftReply, GitLabSeverity, GitLabSide,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    drafts: Vec<GitLabDraft>,
}

pub(crate) struct Drafts {
    path: PathBuf,
    saved: Mutex<Saved>,
}

/// What an agent (or a test) asks to be written. The position is derived, never taken.
#[derive(Debug, Clone)]
pub struct NewDraft {
    pub severity: GitLabSeverity,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub side: GitLabSide,
}

/// Longest body accepted. GitLab's own limit is far higher; this one keeps a runaway agent from
/// writing a megabyte into a file the panel reads whole on every change.
const MAX_BODY: usize = 64 * 1024;

impl Drafts {
    pub(crate) fn load(path: PathBuf) -> Self {
        let saved = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                // Kept aside rather than overwritten: the next save would otherwise replace
                // every unpublished finding with an empty list.
                tracing::warn!(%error, "GitLab drafts did not parse; keeping the file aside");
                let _ = fs::rename(&path, path.with_extension("json.unreadable"));
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Self {
            path,
            saved: Mutex::new(saved),
        }
    }

    fn save(&self, saved: &Saved) -> Result<()> {
        let parent = self.path.parent().ok_or("Invalid GitLab drafts path")?;
        fs::create_dir_all(parent).map_err(|_| "Cannot create GitLab drafts storage")?;
        let tmp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::File::create(&tmp).map_err(|_| "Cannot save GitLab drafts")?;
            file.write_all(&serde_json::to_vec(saved).map_err(|_| "Cannot encode GitLab drafts")?)
                .map_err(|_| "Cannot save GitLab drafts")?;
            file.sync_all().map_err(|_| "Cannot flush GitLab drafts")?;
            fs::rename(&tmp, &self.path).map_err(|_| "Cannot replace GitLab drafts")
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        Ok(result?)
    }

    pub(crate) fn list(&self, review: &str) -> Vec<GitLabDraft> {
        self.saved
            .lock()
            .drafts
            .iter()
            .filter(|d| d.review == review)
            .cloned()
            .collect()
    }

    pub(crate) fn get(&self, review: &str, id: &str) -> Result<GitLabDraft> {
        self.saved
            .lock()
            .drafts
            .iter()
            .find(|d| d.review == review && d.id == id)
            .cloned()
            .ok_or_else(|| format!("No draft `{id}` on this review"))
    }

    pub(crate) fn insert(&self, draft: GitLabDraft) -> Result<GitLabDraft> {
        let mut saved = self.saved.lock();
        saved.drafts.push(draft.clone());
        if let Err(error) = self.save(&saved) {
            saved.drafts.pop();
            return Err(error);
        }
        Ok(draft)
    }

    /// Edit one draft. `run` is the caller's run when an agent asks: a run edits only what it
    /// wrote, so two reviews of one MR cannot rewrite each other's findings.
    pub(crate) fn edit(
        &self,
        review: &str,
        id: &str,
        body: Option<String>,
        severity: Option<GitLabSeverity>,
        run: Option<&str>,
    ) -> Result<GitLabDraft> {
        if let Some(body) = &body {
            check_body(body)?;
        }
        let mut saved = self.saved.lock();
        let before = saved.drafts.clone();
        let draft = saved
            .drafts
            .iter_mut()
            .find(|d| d.review == review && d.id == id)
            .ok_or_else(|| format!("No draft `{id}` on this review"))?;
        owned_by(draft, run)?;
        if let Some(body) = body {
            draft.body = body;
        }
        if let Some(severity) = severity {
            draft.severity = severity;
        }
        let edited = draft.clone();
        if let Err(error) = self.save(&saved) {
            saved.drafts = before;
            return Err(error);
        }
        Ok(edited)
    }

    pub(crate) fn discard(&self, review: &str, ids: &[String], run: Option<&str>) -> Result<usize> {
        let mut saved = self.saved.lock();
        for draft in saved
            .drafts
            .iter()
            .filter(|d| d.review == review && ids.contains(&d.id))
        {
            owned_by(draft, run)?;
        }
        let before = saved.drafts.clone();
        saved
            .drafts
            .retain(|d| !(d.review == review && ids.contains(&d.id)));
        let removed = before.len() - saved.drafts.len();
        if let Err(error) = self.save(&saved) {
            saved.drafts = before;
            return Err(error);
        }
        Ok(removed)
    }

    /// Append one message to a draft's local discussion. `run` is the reviewer's run when an
    /// agent answers: a run answers only on drafts it wrote, for `edit`'s reason — two reviews of
    /// one MR must not talk over each other's findings. The user (`None`) may ask on any draft.
    pub(crate) fn reply(
        &self,
        review: &str,
        id: &str,
        author: GitLabDraftAuthor,
        body: String,
        run: Option<&str>,
    ) -> Result<GitLabDraft> {
        if body.trim().is_empty() {
            return Err("A reply needs some text".into());
        }
        if body.len() > MAX_BODY {
            return Err(format!("A reply is limited to {} KiB", MAX_BODY / 1024));
        }
        let mut saved = self.saved.lock();
        let before = saved.drafts.clone();
        let draft = saved
            .drafts
            .iter_mut()
            .find(|d| d.review == review && d.id == id)
            .ok_or_else(|| format!("No draft `{id}` on this review"))?;
        owned_by(draft, run)?;
        // The reviewer answering from a revived conversation is now in a newer one (a revival
        // forks it); the next revival must resume *that*, or it would not know it had answered.
        if run.is_some() && author.conversation.is_some() {
            draft.author.conversation = author.conversation.clone();
        }
        draft.replies.push(GitLabDraftReply {
            id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
            author,
            body,
            created_unix_ms: now_ms(),
        });
        let replied = draft.clone();
        if let Err(error) = self.save(&saved) {
            saved.drafts = before;
            return Err(error);
        }
        Ok(replied)
    }

    /// Hand every draft `from` wrote to the run `to` — the run a Discuss revived in the same
    /// conversation. Without it the revived reviewer could not answer on, edit or discard the
    /// very findings it is being asked about: `owned_by` compares run ids, and a revival is a
    /// new run. Answers how many drafts moved.
    pub(crate) fn reassign(&self, review: &str, from: &str, to: &str) -> Result<usize> {
        let mut saved = self.saved.lock();
        let before = saved.drafts.clone();
        let mut moved = 0;
        for draft in saved
            .drafts
            .iter_mut()
            .filter(|d| d.review == review && d.author.run.as_deref() == Some(from))
        {
            draft.author.run = Some(to.to_string());
            moved += 1;
        }
        if moved == 0 {
            return Ok(0);
        }
        if let Err(error) = self.save(&saved) {
            saved.drafts = before;
            return Err(error);
        }
        Ok(moved)
    }

    /// Every draft of a closed review goes with it: the review's positions name an MR the user
    /// said they are done with, and a reopened review starts from what GitLab holds.
    pub(crate) fn forget(&self, review: &str) -> Result<()> {
        let mut saved = self.saved.lock();
        if !saved.drafts.iter().any(|d| d.review == review) {
            return Ok(());
        }
        saved.drafts.retain(|d| d.review != review);
        self.save(&saved)
    }
}

fn owned_by(draft: &GitLabDraft, run: Option<&str>) -> Result<()> {
    match run {
        Some(run) if draft.author.run.as_deref() != Some(run) => Err(format!(
            "Draft `{}` was written by {}, not by this run; only its author or the user may change it",
            draft.id, draft.author.label
        )),
        _ => Ok(()),
    }
}

fn check_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err("A draft needs a body".into());
    }
    if body.len() > MAX_BODY {
        return Err(format!(
            "A draft body is limited to {} KiB",
            MAX_BODY / 1024
        ));
    }
    Ok(())
}

/// Build a draft against `version` — the MR version JSON GitLab answers for
/// `…/versions/:id`, diffs included.
pub(crate) fn compose(
    review: &str,
    version: &Value,
    new: NewDraft,
    author: GitLabDraftAuthor,
) -> Result<GitLabDraft> {
    check_body(&new.body)?;
    let head = version["head_commit_sha"]
        .as_str()
        .ok_or("GitLab is still preparing this MR's revisions; try again shortly")?
        .to_string();
    let (path, old_path, side, line, position) = match (new.path, new.line) {
        (None, None) => (None, None, None, None, None),
        (None, Some(_)) => return Err("A line needs a `path` as well".into()),
        (Some(path), None) => {
            return Err(format!(
                "Give a `line` for `{path}`, or leave `path` out for a general comment on the MR"
            ));
        }
        (Some(path), Some(line)) => {
            let change = version["diffs"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["new_path"] == path.as_str() || c["old_path"] == path.as_str())
                .ok_or_else(|| {
                    format!("`{path}` is not changed by this MR; comment only on changed files, or make a general comment")
                })?;
            let position = line_position(change, version, new.side, line).ok_or_else(|| {
                let which = match new.side {
                    GitLabSide::New => "new",
                    GitLabSide::Old => "old",
                };
                if change["diff"].as_str().is_none_or(str::is_empty) {
                    format!("GitLab did not return a diff for `{path}` (too large or collapsed); make a general comment that names the file and line instead")
                } else {
                    format!("Line {line} on the {which} side of `{path}` is not inside the MR diff. GitLab only accepts comments on changed lines and the context lines around them; pick one of those, or make a general comment")
                }
            })?;
            let old_path = change["old_path"].as_str().map(str::to_string);
            (
                Some(path),
                old_path,
                Some(new.side),
                Some(line),
                Some(position),
            )
        }
    };
    Ok(GitLabDraft {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        review: review.to_string(),
        severity: new.severity,
        body: new.body,
        path,
        old_path,
        side,
        line,
        position,
        head_sha: head,
        author,
        created_unix_ms: now_ms(),
        replies: Vec::new(),
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `ui/src/gitlab/model.ts`'s `linePosition`, line for line — the panel's click and an agent's
/// draft must land on the same GitLab coordinates, including a context line carrying both
/// `old_line` and `new_line`, which is what GitLab requires of an unchanged line.
pub(crate) fn line_position(
    change: &Value,
    version: &Value,
    side: GitLabSide,
    number: u32,
) -> Option<Value> {
    let diff = change["diff"].as_str()?;
    let (mut old, mut new) = (0u32, 0u32);
    for text in diff.split('\n') {
        if let Some((o, n)) = hunk_start(text) {
            (old, new) = (o, n);
            continue;
        }
        if old == 0 && new == 0 {
            continue;
        }
        let origin = text.chars().next();
        if !matches!(origin, Some(' ' | '+' | '-')) {
            continue;
        }
        let old_line = (origin != Some('+')).then(|| {
            old += 1;
            old - 1
        });
        let new_line = (origin != Some('-')).then(|| {
            new += 1;
            new - 1
        });
        let hit = match side {
            GitLabSide::Old => old_line,
            GitLabSide::New => new_line,
        };
        if hit == Some(number) {
            let mut position = json!({
                "position_type": "text",
                "base_sha": version["base_commit_sha"],
                "start_sha": version["start_commit_sha"],
                "head_sha": version["head_commit_sha"],
                "old_path": change["old_path"],
                "new_path": change["new_path"],
            });
            if let Some(line) = old_line {
                position["old_line"] = json!(line);
            }
            if let Some(line) = new_line {
                position["new_line"] = json!(line);
            }
            return Some(position);
        }
    }
    None
}

fn hunk_start(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let first = |s: &str| s.split(',').next()?.parse::<u32>().ok();
    Some((first(old)?, first(new)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> Value {
        json!({
            "base_commit_sha": "a".repeat(40),
            "start_commit_sha": "b".repeat(40),
            "head_commit_sha": "c".repeat(40),
            "diffs": [{
                "old_path": "src/old.rs",
                "new_path": "src/new.rs",
                "diff": "@@ -10,3 +10,4 @@ fn x\n context\n-removed\n+added\n+added2\n tail\n",
            }]
        })
    }
    fn author(run: &str) -> GitLabDraftAuthor {
        GitLabDraftAuthor {
            label: "Review !1".into(),
            harness: None,
            run: Some(run.into()),
            conversation: None,
        }
    }
    fn you() -> GitLabDraftAuthor {
        GitLabDraftAuthor {
            label: "You".into(),
            harness: None,
            run: None,
            conversation: None,
        }
    }

    #[test]
    fn a_discussion_is_the_users_on_any_draft_and_a_runs_only_on_its_own() {
        let dir = std::env::temp_dir().join(format!("cide-drafts-{}", uuid::Uuid::new_v4()));
        let path = dir.join("gitlab-drafts.json");
        let store = Drafts::load(path.clone());
        let v = version();
        let draft = store
            .insert(compose("r", &v, new(None, None, GitLabSide::New), author("one")).unwrap())
            .unwrap();
        store
            .reply("r", &draft.id, you(), "Why is this major?".into(), None)
            .unwrap();
        let refused = store
            .reply(
                "r",
                &draft.id,
                author("two"),
                "Not mine".into(),
                Some("two"),
            )
            .unwrap_err();
        assert!(refused.contains("not by this run"), "{refused}");
        assert!(
            store
                .reply("r", &draft.id, you(), "  ".into(), None)
                .is_err()
        );

        // A revival is a new run: until the drafts move to it, it could not answer at all.
        assert_eq!(store.reassign("r", "one", "three").unwrap(), 1);
        store
            .reply(
                "r",
                &draft.id,
                author("three"),
                "Because it drops data.".into(),
                Some("three"),
            )
            .unwrap();

        let reloaded = Drafts::load(path);
        let replies = &reloaded.get("r", &draft.id).unwrap().replies;
        assert_eq!(
            replies.iter().map(|r| r.body.as_str()).collect::<Vec<_>>(),
            ["Why is this major?", "Because it drops data."],
            "the discussion survives a restart, in order"
        );
        assert_eq!(replies[0].author.run, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_drafts_file_from_before_discussions_still_loads() {
        let dir = std::env::temp_dir().join(format!("cide-drafts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gitlab-drafts.json");
        std::fs::write(
            &path,
            r#"{"drafts":[{"id":"d","review":"r","severity":"minor","body":"b","path":null,
                "oldPath":null,"side":null,"line":null,"position":null,"headSha":"h",
                "author":{"label":"Review !1","harness":null,"run":"one"},"createdUnixMs":1}]}"#,
        )
        .unwrap();
        let store = Drafts::load(path.clone());
        let draft = store
            .get("r", "d")
            .expect("an old file is read, not set aside");
        assert!(draft.replies.is_empty());
        assert_eq!(draft.author.conversation, None);
        assert!(!path.with_extension("json.unreadable").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
    fn new(path: Option<&str>, line: Option<u32>, side: GitLabSide) -> NewDraft {
        NewDraft {
            severity: GitLabSeverity::Major,
            body: "Consider this".into(),
            path: path.map(str::to_string),
            line,
            side,
        }
    }

    #[test]
    fn positions_match_the_panels_rule_for_added_removed_and_context_lines() {
        let v = version();
        let change = &v["diffs"][0];
        let context = line_position(change, &v, GitLabSide::New, 10).unwrap();
        assert_eq!(
            (context["old_line"].clone(), context["new_line"].clone()),
            (json!(10), json!(10))
        );
        let removed = line_position(change, &v, GitLabSide::Old, 11).unwrap();
        assert_eq!(removed["old_line"], json!(11));
        assert!(removed.get("new_line").is_none());
        let added = line_position(change, &v, GitLabSide::New, 12).unwrap();
        assert!(added.get("old_line").is_none());
        assert_eq!(added["new_line"], json!(12));
        let tail = line_position(change, &v, GitLabSide::New, 13).unwrap();
        assert_eq!(
            (tail["old_line"].clone(), tail["new_line"].clone()),
            (json!(12), json!(13))
        );
        assert_eq!(added["head_sha"], json!("c".repeat(40)));
        assert_eq!(added["old_path"], json!("src/old.rs"));
        assert!(line_position(change, &v, GitLabSide::New, 40).is_none());
    }

    #[test]
    fn a_line_outside_the_diff_or_file_is_refused_with_a_way_forward() {
        let v = version();
        let outside = compose(
            "r",
            &v,
            new(Some("src/new.rs"), Some(99), GitLabSide::New),
            author("x"),
        )
        .unwrap_err();
        assert!(outside.contains("not inside the MR diff"), "{outside}");
        let unchanged = compose(
            "r",
            &v,
            new(Some("README"), Some(1), GitLabSide::New),
            author("x"),
        )
        .unwrap_err();
        assert!(unchanged.contains("not changed by this MR"), "{unchanged}");
        let general = compose("r", &v, new(None, None, GitLabSide::New), author("x")).unwrap();
        assert!(general.position.is_none());
        let old_path = compose(
            "r",
            &v,
            new(Some("src/old.rs"), Some(11), GitLabSide::Old),
            author("x"),
        )
        .unwrap();
        assert_eq!(old_path.head_sha, "c".repeat(40));
        assert!(
            !old_path.body.contains("major"),
            "the severity is never written into the body"
        );
    }

    #[test]
    fn drafts_round_trip_and_a_run_may_change_only_its_own() {
        let dir = std::env::temp_dir().join(format!("cide-drafts-{}", uuid::Uuid::new_v4()));
        let path = dir.join("gitlab-drafts.json");
        let store = Drafts::load(path.clone());
        let v = version();
        let mine = store
            .insert(
                compose(
                    "r",
                    &v,
                    new(Some("src/new.rs"), Some(12), GitLabSide::New),
                    author("one"),
                )
                .unwrap(),
            )
            .unwrap();
        let theirs = store
            .insert(compose("r", &v, new(None, None, GitLabSide::New), author("two")).unwrap())
            .unwrap();
        store
            .insert(compose("other", &v, new(None, None, GitLabSide::New), author("one")).unwrap())
            .unwrap();

        let reloaded = Drafts::load(path.clone());
        assert_eq!(reloaded.list("r").len(), 2);

        assert!(
            reloaded
                .edit("r", &theirs.id, Some("x".into()), None, Some("one"))
                .is_err()
        );
        let edited = reloaded
            .edit(
                "r",
                &mine.id,
                None,
                Some(GitLabSeverity::Critical),
                Some("one"),
            )
            .unwrap();
        assert_eq!(edited.severity, GitLabSeverity::Critical);
        // The user (no run) may change anything.
        reloaded
            .edit("r", &theirs.id, Some("better".into()), None, None)
            .unwrap();
        assert!(
            reloaded
                .discard("r", std::slice::from_ref(&theirs.id), Some("one"))
                .is_err()
        );
        assert_eq!(
            reloaded.list("r").len(),
            2,
            "a refused discard removes nothing"
        );
        assert_eq!(
            reloaded
                .discard("r", std::slice::from_ref(&theirs.id), None)
                .unwrap(),
            1
        );

        reloaded.forget("r").unwrap();
        assert!(Drafts::load(path).list("r").is_empty());
        assert_eq!(reloaded.list("other").len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
