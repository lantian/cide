//! `.cide/config.json`: whether this project may run subagents, and under what. (M18)
//!
//! # The default that everything else hangs off
//!
//! **`enabled` is false in the absence of the file.** A project that has never heard of this
//! feature can never spawn anything — not after an upgrade, not because a global setting was
//! flipped for a different repository, not because a config file was half-written. Every read
//! path below funnels a missing, unreadable, truncated or unparseable file to
//! [`AgentsConfig::default`], and that default is off.
//!
//! Reading therefore **cannot fail**, and the direction of the failure is the point. Guessing
//! `true` on a file cide could not parse means unattended `claude` processes in somebody's
//! repository, editing files, spending their quota, with the user having done nothing to ask for
//! it. Guessing `false` means a button does not work until they look at the log line. Those are
//! not comparable costs, so a bad parse is `enabled: false` plus a loud `tracing::warn!` naming
//! the file and the serde error — loud because the *other* failure mode of this decision is a
//! user whose config is silently ignored, and a warning with a path in it is what closes that.
//!
//! # Why this is a file in the repository and not a `Settings` field
//!
//! `cide_ipc::OrchestrationConfig`'s doc argues it at length and it is not repeated here; the
//! short of it is that "this project runs subagents" is a property of the **checkout**. It is
//! reviewable in a pull request, where a teammate can see that a repository has started
//! dispatching agents, and above all it must not follow the user into an unrelated project.
//!
//! Two consequences this module is responsible for:
//!
//! * **Nothing here is mirrored into `Workspace`.** The file is read fresh on demand, because a
//!   teammate's commit or a `git checkout` can change it under the running app, and cide's own
//!   state file holding a stale copy of something git owns is a bug with no upper bound on how
//!   long it lasts. (`cide_fs::filter` already watches `<root>/.cide`, so the change arrives.)
//! * **The disk shape is not the wire shape.** `OrchestrationConfig` is what the panel draws —
//!   three fields. [`AgentsConfig`] is what the file holds, and it carries three more,
//!   [`AgentsConfig::isolation`], [`AgentsConfig::allow_dangerous_permissions`] and
//!   [`AgentsConfig::nudge_orchestrator`], which are dispatch-time and turn-end facts with
//!   nothing for a roster row to show. Modelling them here rather than widening the DTO keeps a
//!   switch the panel cannot draw out of the panel's vocabulary — and, for all three, means a
//!   webview round trip cannot reset a key the user hand-edited.

use std::io;
use std::path::{Path, PathBuf};

use cide_ipc::{Harness, OrchestrationConfig, OrchestrationPatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The project directory cide keeps its own files in.
///
/// Spelled here as well as in `cide_fs::filter` because these two crates do not depend on each
/// other and a `pub` constant in either would be a dependency edge added for four characters.
/// It is not a value that will change: it is in users' repositories.
pub const CIDE_DIR: &str = ".cide";

/// `.cide/config.json`'s schema version.
///
/// Written on every save and **not** currently enforced on read: a file from a future version
/// still loads, because `serde(default)` fills what this build does not know and refusing it
/// would break a checkout for a user whose teammate upgraded first. The number is here so a
/// migration, when one is needed, has something to switch on.
pub const SCHEMA_VERSION: u32 = 1;

/// How an agent's work is kept apart from the user's tree.
///
/// # Why a second value exists at all when the answer is always `Worktree`
///
/// Because a project that is not a git repository has nothing to build a worktree on, and the
/// alternative to naming that state is a **silent fallback to the shared tree** — which is how
/// two agents clobber one file with nobody told. `agents_config_set` refuses to enable
/// orchestration with `Worktree` isolation outside a repository, with a sentence; a user who
/// genuinely wants agents editing the working tree directly has to write `"isolation": "shared"`
/// and can be assumed to have read what it means.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Isolation {
    /// One git worktree per (role, task), at `.cide/worktrees/<role>-<task>` on branch
    /// `cide/<role>-<task>` — `crate::run_checkout` is the rule. A run dispatched with no task
    /// stands in the project root even here (M40): a quick check or a small piece of direct
    /// work has nowhere to be merged back *from*.
    #[default]
    Worktree,
    /// Every agent edits the project's own checkout. Nothing separates two concurrent runs.
    Shared,
}

/// The default for [`AgentsConfig::stop_grace_secs`]. See that field for the argument.
pub const DEFAULT_STOP_GRACE_SECS: u16 = 60;

/// The most a project may ask for. Past a few minutes a stop has stopped being a stop, and a
/// typo'd `6000` would pin a role's worktree for most of a day while the row said "winding
/// down". Clamped on read rather than refused: a config that will not load is a checkout that
/// will not open, and [`AgentsConfig`]'s whole loading discipline is that a file from a future
/// or careless version still works.
pub const MAX_STOP_GRACE_SECS: u16 = 600;

/// The default for [`AgentsConfig::auto_spin_after_secs`]. Fifteen minutes.
///
/// Long, because the thing on the other end of this timer is a **billed `claude` process** that
/// starts planning without being asked. Fifteen minutes is past every ordinary lull — a long
/// build, a code review, lunch — and short enough that a project left overnight is picked up
/// rather than abandoned.
pub const DEFAULT_SPIN_AFTER_SECS: u32 = 900;

/// The least a project may ask for, and it is not cosmetic.
///
/// The run-end nudge coalesces with a two-second debounce and a **ten-second ceiling**
/// (`cide_app::agent_rpc`'s `NUDGE_COALESCE`/`NUDGE_CEILING`), so a burst of runs finishing
/// together is still settling for up to ten seconds after the last child exits. A spinner
/// allowed to fire inside that window would look at a project that is between two states and
/// plan for neither. Sixty gives the whole burst room and then some.
pub const MIN_SPIN_AFTER_SECS: u32 = 60;

/// And the ceiling, for [`MAX_STOP_GRACE_SECS`]' reason: a typo'd number must degrade to
/// something a person could have meant. A day.
pub const MAX_SPIN_AFTER_SECS: u32 = 86_400;

/// What a spun `claude` is told, when the project has not written its own.
///
/// **One line, and it has to be**: this is typed into a terminal, where a newline is another
/// Enter — `cide_app::cmd::agents::opening_prompt`'s rule, and the reason the Settings control
/// for it is a single-line field rather than a textarea.
///
/// The shape is *survey, judge, then plan*, in that order, and the order is the whole point.
/// A prompt that opened with "assign the next tasks" would get a model planning from the board's
/// titles alone — which is exactly the state the board is in when nobody has been checking, and
/// is how a project drifts confidently in the wrong direction. So it is told to read what
/// happened first, say plainly whether it is going the right way, and only then decide what
/// happens next.
///
/// **Close before you open** since M83. On a real board (`~/work/selfcraft`) the prompt's
/// *create or re-scope* was read as *create*: 193 of 232 tasks were the planner's, and the board
/// grew faster than it closed. So a step sits between the judgement and the plan — finish what
/// is in review, send to the inbox what does not serve the active milestone — and the plan itself
/// is aimed at the milestone's gate, drawing on the inbox before inventing work. The facts about
/// the gate are appended by `cide_app::spinner::wake`, computed rather than asked for.
pub const DEFAULT_SPIN_PROMPT: &str = "Nothing is running in this project and there is open \
     work on the board, and there is nobody at the keyboard — decide every question yourself \
     from what you can read, and do not end your turn by asking what to do. Before planning \
     anything, survey what has actually happened: read the \
     board with mcp__cide__cide_task_list, read the comments on the tasks in review or doing, \
     check git log and git status to see what landed, and if the project has milestones read \
     them and their gate with mcp__cide__cide_milestones. Then judge it — say plainly whether \
     the work so far is going the way this project needs, and say so even if the answer is no. \
     Then close before you open: merge or hand back what is in review, and move to the inbox \
     (mcp__cide__cide_task_update, status inbox) any todo that does not move the active \
     milestone. Only then plan what happens next — with milestones, only work that makes the \
     active milestone's gate pass, as subtasks of its task, pulling from the inbox \
     (mcp__cide__cide_task_list with status inbox and a query) before writing anything new; \
     create or re-scope tasks with mcp__cide__cide_task_create and \
     mcp__cide__cide_task_update, and put them on the roles mcp__cide__cide_agents_list knows \
     about with mcp__cide__cide_task_assign, which is what starts them working.";

/// What a reviewer tab is told when a subagent's turn on a task ends, when the project has not
/// written its own. A **template**: `{name}` placeholders are filled by [`fill_review_prompt`]
/// with the facts of the run being reviewed, and [`REVIEW_PLACEHOLDERS`] is every name it knows.
///
/// Per project because what "done and correct" means is the repository's — which checks to run,
/// what the base branch is, how strict to be — and the user asked to edit it beside the switch
/// that opens the tab. It lived in `cide_app::agent_rpc::review_prompt` as a `format!` until then;
/// that function still carries the argument for every clause, and still owns the task-less case,
/// which has no task, branch or role to fill a template with.
///
/// One line for [`DEFAULT_SPIN_PROMPT`]'s reason: it is typed into a terminal.
pub const DEFAULT_REVIEW_PROMPT: &str = "A subagent just {outcome}: `{agent}`, on {task}. You \
     own that task now — nobody else is reviewing it and the product owner has not been told, so \
     it is yours to finish or to send back. **There is nobody at the keyboard and nobody will \
     answer you**: decide every question yourself from what you can read, and never end your \
     turn by asking what to do — a turn that ends in a question is a task nobody picks up. First \
     find out what actually happened: read the task with mcp__cide__cide_task_get and read its \
     comments, which are the only place the run reported; then diff the branch {branch} against \
     the base to see what changed rather than what was claimed, and run whatever this repository \
     uses to check itself. Then take one of exactly two actions. If the work is done and correct \
     and did what the task asked rather than something adjacent: comment your verdict with \
     mcp__cide__cide_task_comment, then **merge it yourself** with \
     mcp__cide__cide_agent_integrate (agent `{role}`, task {task_id}) — accepting work means \
     taking it into this branch, and leaving a merge for somebody to do later is the same as not \
     accepting it — and only then set the task to done with mcp__cide__cide_task_update. If that \
     merge reports conflicts it has changed nothing, and the task is not done: treat it as the \
     other case below, naming the conflicting paths. If the merge does not happen for any other \
     reason — an error, or the call itself denied by a permission check — the task is not done \
     either: leave it in review, comment exactly what stopped the merge, and do not set it to \
     done (cide refuses done while the branch is unmerged); review is where the user looks, and \
     done is where nobody does. If it says there was nothing to merge, that \
     is a normal answer and not a problem to report — the branch is already in, usually because \
     an earlier review merged it — so check the work is present and close the task. If the work \
     is not right: comment exactly what is wrong and what is still needed, set the task back to \
     doing with mcp__cide__cide_task_update, and hand it back to the same role with \
     mcp__cide__cide_agent_dispatch (agent `{role}`, task {task_id}) passing that same feedback \
     as the instructions — do not fix it yourself, the role that built it has the context. One \
     exception, and read the comments for it before you dispatch: if this task has already been \
     sent back for the same reason, stop, say so plainly in a comment, leave it in review and do \
     not dispatch again. When the project configures a verify command, \
     mcp__cide__cide_agent_integrate runs it on the branch first and refuses a failing one, \
     quoting the output: that refusal is the not-right case, with its feedback already written. \
     Anything you notice that is wrong but is *not* this task — something the run broke \
     elsewhere, a gap it revealed, work this one turns out to depend on — goes on the board as \
     its own task with mcp__cide__cide_task_create and `inbox: true`, rather than into this \
     task's verdict where it is read once and lost; the inbox is where noticed work waits until \
     whoever plans decides it is needed.{others}";

/// Every placeholder [`fill_review_prompt`] fills, with what it becomes. The Settings hint lists
/// the same names; `every_placeholder_is_filled` holds the two ends of this table together.
pub const REVIEW_PLACEHOLDERS: &[(&str, &str)] = &[
    ("task_id", "the task's id, e.g. t-17"),
    (
        "task_title",
        "the task's title, clipped; empty when it could not be read",
    ),
    (
        "task",
        "`task t-17 (its title)`, or `task t-17` without one",
    ),
    ("branch", "the run's branch, e.g. cide/developer-t-17"),
    (
        "role",
        "the role's id, which is what dispatch and integrate answer to",
    ),
    ("agent", "the role's label, which is what a person reads"),
    (
        "outcome",
        "how the turn ended: handed its turn back, finished (exit 0), failed",
    ),
    (
        "others",
        "a sentence about other turns that ended in the same moment, or nothing",
    ),
];

/// Fill a review template. (M84)
///
/// **One pass, left to right, and a filled value is never scanned again.** The values are a task
/// title any agent may write and a label out of a committed markdown file; a `str::replace` per
/// placeholder would expand a title that says `{branch}` on the next replace, so text nobody in
/// the project wrote would be choosing what the reviewer is told. An unknown `{name}` — a typo,
/// or a brace that was never a placeholder — is left exactly as written, which is what somebody
/// reading their own prompt back would expect and costs nothing.
///
/// The result is flattened to one line, for the terminal's reason, whatever the values held.
#[must_use]
pub fn fill_review_prompt(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 128);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let filled = after.find('}').and_then(|close| {
            let name = &after[..close];
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value, close))
        });
        match filled {
            Some((value, close)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What [`AgentsConfig::permission_mode`] may say. Checked in order by nothing; listed for the
/// sentence a bad value produces.
pub const UNATTENDED_MODES: &[&str] = &["auto", crate::defs::BYPASS_PERMISSIONS, "manual"];

/// The project's stance for an unattended child whose role names no permission mode. (M82)
///
/// Three values and not the whole of [`crate::defs::PERMISSION_MODES`]. A project default is
/// mapped onto every harness, and `plan` or `acceptEdits` as a default for *every* role would
/// be a restriction nobody can see from the role that hits it. A role that wants one of those
/// names it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unattended {
    /// Claude's classifier mode. The default. See [`AgentsConfig::permission_mode`] for what
    /// it becomes on a harness without one.
    #[default]
    Auto,
    /// No prompts and no classifier.
    Bypass,
    /// The CLI's own default, which asks. Nothing is passed.
    Ask,
}

/// The `agents` block of `.cide/config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentsConfig {
    /// Whether this project may dispatch subagents at all. See the module header.
    pub enabled: bool,
    /// How many runs may be live across the whole project, whatever a role's own
    /// `max-concurrent` says.
    ///
    /// 2, matching `OrchestrationConfig::default`, and for the reason its doc gives: every run is
    /// a full `claude` with its own context window and its own bill, and a default that started
    /// six of them the first time somebody pressed the button is a default nobody forgives.
    pub max_concurrent: u16,
    /// The harness a role gets when its own definition does not name one.
    ///
    /// Per project rather than global, because which CLI a team runs is a property of the team's
    /// repository — and because a role definition is committed beside this file, so the two are
    /// reviewed together.
    pub harness: Harness,
    /// Whether agents get their own git worktree. Not on the wire; see [`Isolation`].
    pub isolation: Isolation,
    /// Whether a role declaring `permission-mode: bypassPermissions` may actually be dispatched.
    ///
    /// # Why this is a separate switch and not just the role's own setting
    ///
    /// `bypassPermissions` means an unattended process edits, deletes and runs whatever it likes
    /// with nothing to stop it. A role's own definition file is *not* a sufficient place to
    /// authorise that: definition files are committed, so one arrives with a `git pull`, and they
    /// are routinely written by models. Requiring a second, explicit opt-in **in the project's
    /// own config** means the dangerous combination takes two deliberate acts by the person whose
    /// machine it is.
    ///
    /// The refusal is at **dispatch**, never at load — see `crate::dispatch_refusal`. Refusing at
    /// load would make the role vanish from the roster, and a user staring at a missing agent has
    /// no thread to pull; a greyed row that names this key is a fix they can act on.
    ///
    /// A project whose own default is `bypassPermissions` ([`Self::permission_mode`]) is the
    /// stated exception. It has already declared that every unattended child runs with no brake,
    /// so refusing a role for writing down the same stance would be a refusal about nothing.
    /// Under the default `auto` the exception does not apply, because a project on `auto` has
    /// not agreed to bypass.
    pub allow_dangerous_permissions: bool,
    /// Whether a subagent finishing its turn types one line into the product owner's terminal.
    ///
    /// # The most invasive thing in this design, and it ships on
    ///
    /// A run reports back only through `.cide/tasks.json`, and there is **no out-of-band channel
    /// into a running `claude`** — nothing that can hand a live session a message which is not a
    /// keystroke. So the only way to tell a project's primary session that one of its subagents
    /// has finished a turn is to write into that session's PTY, exactly as a dispatch writes the
    /// opening prompt into a run's: a line appears in the user's own conversation and is
    /// submitted as a turn, with nobody at the keyboard.
    ///
    /// That is worth being uncomfortable about, and the discomfort is the whole reason this key
    /// exists. It is nevertheless **true by default**, because the loop it closes — decompose,
    /// dispatch, read what came back, dispatch the next thing — is the feature M18 was asked for,
    /// and a loop whose last step is *and then the user happens to notice* is not a loop. **The
    /// setting is the way out, not the way in.** A project that wants subagents but does not want
    /// its console typed into writes `"nudgeOrchestrator": false` here and loses nothing it
    /// cannot get back by asking: `mcp__cide__cide_agent_runs` answers the same question, and
    /// answers it as of the moment it is called rather than as of the last thing that happened.
    ///
    /// Disk-only, like [`Self::isolation`] and [`Self::allow_dangerous_permissions`] and for the
    /// same reason — it is deliberately absent from `OrchestrationConfig`, so no webview gesture
    /// can set it and no round trip through the panel can silently reset it either. It is also
    /// read **fresh at the moment of each nudge** rather than cached when a run is dispatched:
    /// this file is committed, so a teammate's commit or a `git checkout` can switch it off under
    /// a running app, and a cached copy would go on typing into somebody who had already said no.
    pub nudge_orchestrator: bool,
    /// Whether assigning a task to a role — or @mentioning one in a task's body or a comment —
    /// starts that role working on it.
    ///
    /// `cide_agents::autodispatch` is the policy (who may trigger, which statuses, what a
    /// mention does); this key is the master switch over all of it. It exists for
    /// [`Self::nudge_orchestrator`]'s reason, sharpened: that key types a line into a terminal,
    /// this one **spawns a billed `claude` process off a task edit**. On by default because the
    /// loop it closes — plan on the board, assign, work starts — is what assignment is *for*;
    /// the setting is the way out, not the way in. A project that wants assignment to stay pure
    /// bookkeeping writes `"autoDispatch": false` here and keeps `cide_agent_dispatch` as the
    /// explicit road.
    ///
    /// Disk-only and read fresh at each trigger, exactly like `nudge_orchestrator` and for the
    /// same two reasons: no panel round trip can silently reset it, and a teammate's commit
    /// switching it off is honoured from the next edit onward.
    pub auto_dispatch: bool,
    /// The permission mode an unattended child runs under when its role names none. (M82)
    ///
    /// One of [`UNATTENDED_MODES`]: `"auto"` (the default), `"bypassPermissions"` or
    /// `"manual"`. [`Self::unattended`] reads it, and it is the only reader. The role's own
    /// `permission-mode:` always wins over this value, and a local override
    /// (`cide_ipc::AgentOverride::permission_mode`) wins over both.
    ///
    /// # Why an unattended child must not prompt, measured
    ///
    /// Nobody answers a prompt in an unattended child. Measured per harness: `opencode run`
    /// auto-rejects the request and the **turn ends right there**. Two real runs died that way
    /// on their first out-of-project command with the task never reported, which reads on the
    /// board as an agent that did nothing. A `claude` run parks in `AwaitingPermission`, holding
    /// its slot and its role's only worktree, until a human happens to open its pane. Neither is
    /// a safety property; in both the autonomy fails silently. `"manual"` is therefore a
    /// coherent choice only for a project that wants to watch every run.
    ///
    /// # Why `auto` and not `bypassPermissions`
    ///
    /// From M67 to M81 this was the boolean `skipPermissions`, and on meant
    /// `bypassPermissions`: a child that edits, deletes and runs anything, contained only by its
    /// worktree. Claude's `auto` mode keeps that autonomy and puts a classifier in front of each
    /// action. The cost is stated rather than hidden: when the classifier will not approve
    /// something, the CLI can fall back to asking, and an unattended run that asks parks exactly
    /// as above. A run is expected to stall occasionally, which is a far smaller hazard than a
    /// child with no brake at all.
    ///
    /// `auto` is Claude's word, and not every harness has a counterpart that keeps a headless
    /// run working. codex's `--approve-for-me` keeps the `workspace-write` sandbox, which
    /// re-binds `.git` read-only, so the run cannot `git commit`. opencode and mimo have one
    /// switch in total. So on those harnesses the project default `auto` keeps the promptless
    /// flag it always had, and each harness's own `unattended` mapping says so where it happens.
    /// A *role* that writes `permission-mode: auto` still gets each CLI's literal mapping,
    /// because the role's author chose that trade.
    ///
    /// # Reading the old key
    ///
    /// A file with no `permissionMode` falls back to [`Self::skip_permissions`]. `false` means
    /// `"manual"`, because a project that switched prompts back on said so on purpose and an
    /// upgrade must not re-arm its children. `true` means `"auto"`, **not** bypass: every file
    /// cide wrote before M82 carries `"skipPermissions": true` whether anybody chose it or not,
    /// so the key cannot be read as a choice of bypass. A project that wants bypass writes
    /// `"permissionMode": "bypassPermissions"`.
    ///
    /// Absent on disk while `None`. If the default were written out, the panel's first save would
    /// put `"permissionMode": "auto"` next to a hand-written `"skipPermissions": false` and
    /// silently outrank it. For the same reason, [`write`] never adds the key on its own.
    ///
    /// Disk-only like its neighbours, and read fresh at each spawn, so a `git checkout` that
    /// changes it takes effect from the next dispatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// The key [`Self::permission_mode`] replaced. Read for its migration and nothing else, and
    /// never written unless the file already had it — a key the merge in [`write`] would
    /// preserve anyway.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_permissions: Option<bool>,
    /// How long a stopped run is given to wind itself down before cide ends it. Seconds.
    ///
    /// # Why a stop waits at all, and why the number lives here
    ///
    /// `cide_agent_stop` used to kill outright, and an agent forty minutes into a task usually
    /// knows things it has not committed or commented — a measurement just taken, a dead end
    /// ruled out, a defect noticed in passing. All of it died with the child, and the board was
    /// left with `exit 129`, which a crash produces too. So a stop now *asks* first: the run is
    /// told to stop, given the caller's reason in their own words, and asked to write what it
    /// finished and what it was part-way through into its task before exiting. This is how long
    /// it has.
    ///
    /// Sixty, because the wind-down is one more **turn** and not one more line: a model round
    /// trip plus a tracker call, on a child that may be mid-tool-call when it is asked. Ten
    /// seconds is a deadline nothing meets, and would make every stop a force with extra steps.
    ///
    /// **`0` is allowed and means this project forces every stop** — the pre-M67 behaviour, and
    /// a coherent policy for a team whose agents have nothing to hand over. It is deliberately
    /// *not* clamped up to 1 the way [`Self::max_concurrent`] is: a zero concurrency defines a
    /// project that can never dispatch, which is nonsense rather than policy, while a zero grace
    /// says something a person could mean. The **ceiling** is clamped on read instead, because
    /// past a few minutes a stop has stopped being a stop.
    ///
    /// Disk-only beside its three neighbours and for their reason — no webview gesture can set
    /// it, so a round trip through the panel cannot silently reset a number somebody hand-edited
    /// — and **read fresh at the moment of each stop**, never cached at dispatch: this file is
    /// committed, so a teammate's commit or a `git checkout` can change it under a running app,
    /// and a deadline computed at dispatch would honour a number nobody now has.
    pub stop_grace_secs: u16,
    /// Whether a subagent's turn ending opens a **new Claude tab** to review it, instead of
    /// typing one line into the product owner's conversation. (M79)
    ///
    /// # What this is actually fixing
    ///
    /// [`Self::nudge_orchestrator`]'s line lands in the conversation that *dispatched* the work,
    /// which by the time a long run ends may be a hundred turns deep in something else. The line
    /// then has to compete for that context's attention against everything already in it, and
    /// what it is asking for — go and check somebody else's work — is exactly the kind of task a
    /// loaded context does worst.
    ///
    /// So the note goes to a `claude` that has nothing else in its head: a fresh `ClaudeFull`
    /// tab, opened by cide, carrying a prompt that names the task, the run and the branch and
    /// asks for a verdict on the task. The tab stays until somebody closes it, which is what
    /// makes the review a thing on screen rather than a line that scrolled past.
    ///
    /// **It replaces the console line rather than joining it.** Two announcements of one fact
    /// would cost two turns and give two readers the same job, and the reader with the empty
    /// context is the better one.
    ///
    /// On by default, for [`Self::auto_dispatch`]'s reason: the loop this closes is the feature,
    /// and a loop whose last step is *and then somebody notices* is not a loop. It is
    /// nevertheless **under** `nudge_orchestrator`, not beside it — a project that said *do not
    /// announce my subagents to anybody* has not asked for a tab either, so that key still
    /// silences both roads.
    pub finish_in_new_tab: bool,
    /// Whether cide wakes a quiet project up by itself. (M79)
    ///
    /// # The most autonomous thing in this file, and it ships off
    ///
    /// Every other switch here reacts to something a person or an agent *did*: a task was
    /// assigned, a run ended, a stop was asked for. This one reacts to **nothing happening** —
    /// after [`Self::auto_spin_after_secs`] of a project with open tasks, no live run and no
    /// working Claude pane, cide spawns a `claude` and hands it [`Self::auto_spin_prompt`].
    ///
    /// That is a billed process started by a timer, on a machine whose owner may not be at it,
    /// and it is the one key in this struct that is **off by default** on the same reasoning as
    /// [`Self::enabled`]: a default that spends money while nobody is watching is a default
    /// nobody forgives. The way in is deliberate and the way out is the same switch.
    ///
    /// It is also gated by everything else: `enabled` must be on, the queue must not be paused
    /// (`AgentRegistry::dispatching`), and the whole of `cide_app::spinner::should_spin` has to
    /// agree. See that function — it is pure, and it is where the rules live.
    pub auto_spin: bool,
    /// How long a project must be quiet before [`Self::auto_spin`] fires. Seconds.
    ///
    /// A **dwell**, not an interval: the clock restarts every time the project looks busy, so
    /// this is "nothing has happened for this long" rather than "fire every this often". A bare
    /// interval would land in the middle of a settling burst.
    ///
    /// Clamped on read through [`Self::spin_after`], which is the only reader, so the floor and
    /// the ceiling cannot be forgotten at a second call site. The floor is not cosmetic — see
    /// [`MIN_SPIN_AFTER_SECS`].
    pub auto_spin_after_secs: u32,
    /// What the spun `claude` is told. One line; see [`DEFAULT_SPIN_PROMPT`].
    ///
    /// Per project rather than global because it is about *this* repository's work — what
    /// "going the right way" means here, which roles exist, what to look at first. A blank or
    /// whitespace-only value reads as the default rather than as an empty prompt, because a
    /// `claude` handed a lone Enter starts a turn about nothing and bills for it;
    /// [`Self::spin_prompt`] is the one reader that decides this.
    pub auto_spin_prompt: String,
    /// What a reviewer tab opened by [`Self::finish_in_new_tab`] is told. A template; see
    /// [`DEFAULT_REVIEW_PROMPT`] and [`fill_review_prompt`]. Blank reads as the default, on
    /// [`Self::auto_spin_prompt`]'s argument — [`Self::review_prompt_template`] decides it.
    pub review_prompt: String,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            // The single most important line in this crate.
            enabled: false,
            max_concurrent: 2,
            harness: Harness::Claude,
            isolation: Isolation::Worktree,
            allow_dangerous_permissions: false,
            // On, and the field's own doc argues it: the setting is the way out, not the way in.
            nudge_orchestrator: true,
            // Same argument, same default: assignment that does nothing is not assignment.
            auto_dispatch: true,
            // Absent, which reads as `auto`: see `unattended`. Neither key is written, so the
            // panel's first save cannot outrank a hand-written legacy `skipPermissions: false`.
            permission_mode: None,
            skip_permissions: None,
            // A minute: one more turn, including a tool call. The field's doc argues it.
            stop_grace_secs: DEFAULT_STOP_GRACE_SECS,
            // On: the field's doc argues it, and it is still under `nudge_orchestrator`.
            finish_in_new_tab: true,
            // **Off**, and the only key here besides `enabled` that is. A timer that spends
            // money while nobody is watching is the one default this file may not get wrong.
            auto_spin: false,
            auto_spin_after_secs: DEFAULT_SPIN_AFTER_SECS,
            auto_spin_prompt: DEFAULT_SPIN_PROMPT.to_string(),
            review_prompt: DEFAULT_REVIEW_PROMPT.to_string(),
        }
    }
}

impl AgentsConfig {
    /// How long a stop waits, as a duration, with the ceiling applied. (M67)
    ///
    /// The one reader of [`Self::stop_grace_secs`], so the clamp cannot be forgotten at a second
    /// call site and the two answers cannot disagree. `Duration::ZERO` is a real answer and
    /// means *force every stop* — the caller branches on it rather than treating it as absent.
    #[must_use]
    pub fn stop_grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.stop_grace_secs.min(MAX_STOP_GRACE_SECS)))
    }

    /// How long the spinner waits, as a duration, with both bounds applied. (M79)
    ///
    /// The one reader of [`Self::auto_spin_after_secs`], on [`Self::stop_grace`]'s rule: a clamp
    /// with two call sites is a clamp that disagrees with itself. Unlike `stop_grace` there is no
    /// meaningful zero here — `0` would mean *spin the instant a project goes quiet*, which fires
    /// inside the nudge coalescer's own settling window and is the one value
    /// [`MIN_SPIN_AFTER_SECS`] exists to refuse — so this clamps up as well as down.
    #[must_use]
    pub fn spin_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(
            self.auto_spin_after_secs
                .clamp(MIN_SPIN_AFTER_SECS, MAX_SPIN_AFTER_SECS),
        ))
    }

    /// What to type into a spun run, with the blank case decided once. (M79)
    ///
    /// The one reader of [`Self::auto_spin_prompt`]. A blank or whitespace-only value is the
    /// default and never an empty prompt: the alternative is a `claude` handed a lone Enter,
    /// which starts a turn about nothing, bills for it, and leaves a tab whose only content is
    /// the model asking what was wanted. Clearing the box in Settings therefore means *use the
    /// one cide ships*, which is also the only thing a person clearing it could mean.
    #[must_use]
    pub fn spin_prompt(&self) -> &str {
        if self.auto_spin_prompt.trim().is_empty() {
            DEFAULT_SPIN_PROMPT
        } else {
            &self.auto_spin_prompt
        }
    }

    /// The review template to fill, with the blank case decided once — [`Self::spin_prompt`]'s
    /// rule, for its reason: an empty prompt is a billed turn about nothing.
    #[must_use]
    pub fn review_prompt_template(&self) -> &str {
        if self.review_prompt.trim().is_empty() {
            DEFAULT_REVIEW_PROMPT
        } else {
            &self.review_prompt
        }
    }

    /// The stance for an unattended child whose role names no mode. The one reader of
    /// [`Self::permission_mode`] and [`Self::skip_permissions`], so the migration rule cannot be
    /// applied at one call site and forgotten at another.
    ///
    /// An unrecognised word reads as [`Unattended::Ask`], with a warning that names it. Ask is
    /// the direction a typo in the switch for unattended processes must fail in, and a
    /// run that then parks on a prompt is visible where a silently widened one is not.
    #[must_use]
    pub fn unattended(&self) -> Unattended {
        match self.permission_mode.as_deref().map(str::trim) {
            Some("auto") => Unattended::Auto,
            Some(crate::defs::BYPASS_PERMISSIONS) => Unattended::Bypass,
            Some("manual") => Unattended::Ask,
            Some(other) => {
                tracing::warn!(
                    value = other,
                    allowed = UNATTENDED_MODES.join(", "),
                    "agents.permissionMode in .cide/config.json is not a mode cide knows; \
                     unattended runs will ask"
                );
                Unattended::Ask
            }
            None => match self.skip_permissions {
                Some(false) => Unattended::Ask,
                Some(true) | None => Unattended::Auto,
            },
        }
    }

    /// The fields the panel draws.
    pub fn to_wire(&self) -> OrchestrationConfig {
        OrchestrationConfig {
            enabled: self.enabled,
            max_concurrent: self.max_concurrent,
            harness: self.harness,
            finish_in_new_tab: self.finish_in_new_tab,
            auto_spin: self.auto_spin,
            // The **stored** number and not `spin_after()`'s clamped one, deliberately. The box
            // has to show what the file says, or a hand-written value outside the bounds is
            // silently rewritten the first time somebody opens Settings and tabs past the field
            // — `NumberField` commits on blur. The clamp belongs at the point of use.
            auto_spin_after_secs: self.auto_spin_after_secs,
            // Likewise the stored string, not `spin_prompt()`: drawing the default into a box
            // the file left empty would make the next blur write it to disk, turning "use
            // whatever cide ships" into a frozen copy of this build's wording.
            auto_spin_prompt: self.auto_spin_prompt.clone(),
            // The stored string, for `auto_spin_prompt`'s reason above.
            review_prompt: self.review_prompt.clone(),
        }
    }

    /// Apply a wire patch. `None` means "leave this alone", per `OrchestrationPatch`'s contract.
    ///
    /// The three disk-only fields are untouched by any patch, deliberately: `isolation`,
    /// `allow_dangerous_permissions` and `nudge_orchestrator` are not on the wire, so a UI
    /// gesture cannot set them and a round trip through the panel cannot silently reset them
    /// either. They are hand-edited, and [`write`] preserves what it did not change.
    pub fn apply(&mut self, patch: OrchestrationPatch) {
        if let Some(enabled) = patch.enabled {
            self.enabled = enabled;
        }
        if let Some(max) = patch.max_concurrent {
            // Clamped rather than refused: this arrives from a webview, and the whole point of
            // `cmd::settings::apply_patch`'s clamps is that a nonsense number should land as a
            // sane one rather than as an error the frontend discards. 0 would define a project
            // that can never dispatch, which no user means.
            self.max_concurrent = max.max(1);
        }
        if let Some(harness) = patch.harness {
            self.harness = harness;
        }
        if let Some(tab) = patch.finish_in_new_tab {
            self.finish_in_new_tab = tab;
        }
        if let Some(spin) = patch.auto_spin {
            self.auto_spin = spin;
        }
        if let Some(secs) = patch.auto_spin_after_secs {
            // Clamped here as well as at `spin_after`, and the two are not the same act.
            // `spin_after` protects the *timer* from whatever the file happens to hold,
            // including a value some other tool wrote. This protects the **file** from a
            // webview, so what the panel draws back is what the timer will use — `max_concurrent`
            // above makes the same trade for the same reason.
            self.auto_spin_after_secs = secs.clamp(MIN_SPIN_AFTER_SECS, MAX_SPIN_AFTER_SECS);
        }
        if let Some(prompt) = patch.auto_spin_prompt {
            // Flattened on the way in, never at the point of use. This string is typed into a
            // terminal, where a newline is a second Enter that submits the tail of the prompt as
            // its own turn — `opening_prompt`'s finding 8. Doing it here means the file can never
            // hold a prompt that would misbehave, and the box shows exactly what will be sent;
            // flattening at the spawn instead would leave a config somebody read as two lines
            // and a run that silently got one.
            self.auto_spin_prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        if let Some(prompt) = patch.review_prompt {
            // Flattened on the way in, `auto_spin_prompt`'s rule and reason.
            self.review_prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }
}

/// `.cide/config.json`, whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CideConfig {
    pub version: u32,
    pub agents: AgentsConfig,
}

impl Default for CideConfig {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            agents: AgentsConfig::default(),
        }
    }
}

/// `<root>/.cide`.
pub fn cide_dir(project_root: &Path) -> PathBuf {
    project_root.join(CIDE_DIR)
}

/// `<root>/.cide/config.json`.
///
/// Named in full in `AgentRoster::Disabled`, and *before* the enable button, because turning
/// this on writes a file the user's repository will then contain — and a feature toggle that
/// quietly adds a committed file is a surprise commit.
pub fn config_path(project_root: &Path) -> PathBuf {
    cide_dir(project_root).join("config.json")
}

/// Read a project's config. Never fails; see the module header for why that is not laziness.
pub fn load(project_root: &Path) -> CideConfig {
    load_file(&config_path(project_root))
}

/// [`load`] against an explicit path.
pub fn load_file(path: &Path) -> CideConfig {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // The overwhelmingly common case: no file, no feature, nothing to say. Not even a
            // debug line — it would fire for every project the user has ever opened.
            return CideConfig::default();
        }
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "could not read .cide/config.json; subagents stay disabled for this project"
            );
            return CideConfig::default();
        }
    };
    match serde_json::from_slice::<CideConfig>(&bytes) {
        Ok(config) => config,
        Err(err) => {
            // Loud, with the path and the line serde found the problem on. The whole file is
            // discarded rather than partially applied: serde has already stopped, and a config
            // that is half the user's intent and half the defaults is the shape nobody can
            // reason about.
            tracing::warn!(
                path = %path.display(),
                %err,
                "\
                .cide/config.json could not be parsed; subagents stay disabled for this project"
            );
            CideConfig::default()
        }
    }
}

/// A project's milestones: the `milestones` key of `.cide/config.json`. (M83)
///
/// Read **separately** from [`load`], and that is the point of it being its own function. `load`
/// parses the whole file into [`CideConfig`] and discards all of it on any error — right for
/// `agents`, where a half-understood config is worse than none, and wrong here: a typo in one
/// milestone's gate would otherwise switch subagents off for the whole project with nothing but a
/// log line to say why. [`CideConfig`] does not carry this key at all, so [`write`] (which merges
/// into the existing document) leaves it exactly as it found it and [`write_milestones`] is its
/// one writer.
pub fn load_milestones(project_root: &Path) -> cide_ipc::MilestonePlan {
    let path = config_path(project_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return cide_ipc::MilestonePlan::default();
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else {
        return cide_ipc::MilestonePlan::default();
    };
    match doc.get("milestones") {
        None | Some(Value::Null) => cide_ipc::MilestonePlan::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    %err,
                    "the milestones in .cide/config.json could not be read; this project has none \
                     until they are fixed"
                );
                cide_ipc::MilestonePlan::default()
            }
        },
    }
}

/// Replace the `milestones` key, leaving every other key as it was. (M83)
///
/// Whole-value, not a merge: the list is ordered and an item removed in Settings must go, which
/// is the decision [`merge_object`]'s doc defers to whoever first puts an array in this file.
/// An empty plan removes the key rather than writing `{"items": []}` into a committed file.
pub fn write_milestones(project_root: &Path, plan: &cide_ipc::MilestonePlan) -> io::Result<()> {
    let path = config_path(project_root);
    let mut doc = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({ "version": SCHEMA_VERSION }));
    let map = doc.as_object_mut().expect("filtered to an object above");
    if plan.is_empty() && plan.verify.trim().is_empty() && plan.guard_paths.is_empty() {
        map.remove("milestones");
    } else {
        map.insert(
            "milestones".to_string(),
            serde_json::to_value(plan).map_err(io::Error::other)?,
        );
    }
    let mut body = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
    body.push(b'\n');
    write_0644(&path, &body)
}

/// Write a project's config, creating `.cide/` if it is not there.
///
/// # What this preserves, and why it is not just `to_vec_pretty(config)`
///
/// The file is **committed and hand-edited**, so it is not cide's to own outright. Two things
/// follow. Keys this build does not know — a field from a newer cide that a teammate's commit
/// introduced — are kept, by merging into the file's existing JSON rather than replacing it;
/// dropping them would mean two people with different versions silently reverting each other's
/// config in alternate commits. And key order is kept, because `serde_json` is built here with
/// `preserve_order`, so a save does not reshuffle a file somebody arranged.
///
/// (A file that could not be parsed at all is replaced. There is nothing in it to preserve that
/// could be preserved *correctly*, and it is already being ignored by [`load_file`].)
///
/// # Why the mode is 0644 and not `persist::write_atomic`'s 0600
///
/// `cide_core::persist::write_atomic` creates at **0600**, and its doc explains why at length:
/// `workspace.json` holds `Settings::proxy`, which can be `http://user:hunter2@proxy:3128`, and
/// a 0644 there is that password readable by every account on the machine. **None of that
/// applies here.** This file lands in the user's repository, is committed, is read by their
/// teammates and is checked out by CI; a 0600 would show up as a mode change in `git status` on
/// a repository where every other file is 0644, and would make the file unreadable to a
/// different user running a build in the same checkout. So the mode differs deliberately, and
/// that is exactly why this cannot call `write_atomic` — the one thing that function's doc says
/// nobody should be re-deciding is the part that has to change.
///
/// Everything else about `write_atomic`'s shape is kept: a sibling temp file (rename is only
/// atomic within one filesystem), `sync_all` before the rename, the directory synced after it,
/// and the temp removed on every failure path. A torn write here would be a config file
/// committed half-written.
pub fn write(project_root: &Path, config: &CideConfig) -> io::Result<()> {
    let path = config_path(project_root);

    let mut doc = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    let ours = serde_json::to_value(config).map_err(io::Error::other)?;
    merge_object(&mut doc, &ours);

    let mut body = serde_json::to_vec_pretty(&doc).map_err(io::Error::other)?;
    // A committed text file without a trailing newline shows as "\ No newline at end of file" in
    // every diff of it, for ever.
    body.push(b'\n');
    write_0644(&path, &body)
}

/// Overwrite the keys of `ours` into `doc`, one level deep on nested objects.
///
/// One level is exactly what this file needs (`version`, and the `agents` object) and stopping
/// there is deliberate: a general deep merge would have to decide what to do about arrays, and
/// this file has none. If it grows one, that decision gets made then, with the array in front of
/// whoever makes it.
fn merge_object(doc: &mut Value, ours: &Value) {
    let (Some(target), Some(source)) = (doc.as_object_mut(), ours.as_object()) else {
        *doc = ours.clone();
        return;
    };
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(existing), Value::Object(_)) if existing.is_object() => {
                merge_object(existing, value);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Publish `body` at `path`, world-readable. See [`write`]'s doc for the mode.
fn write_0644(path: &Path, body: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;

    // The pid keeps two cide processes apart; the counter keeps two threads of one apart, which
    // the pid alone does not. Sharing a temp name is not a lost race but a corrupt file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp-{}-{seq}", std::process::id()));

    let write = (|| -> io::Result<()> {
        let mut file = create_shared(&tmp)?;
        file.write_all(body)?;
        // Without this the rename can publish an intact name over contents that never reached
        // the disk — which is the crash this dance exists to survive.
        file.sync_all()
    })();
    if let Err(err) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        // Leaving it behind would accumulate one stray file per failed save, inside a directory
        // the user commits.
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    // The rename is itself a directory modification, and an unsynced one is lost in the same
    // crash.
    let _ = std::fs::File::open(dir).map(|d| d.sync_all());
    Ok(())
}

/// Create the temp file at 0644, whatever the umask says.
///
/// Two steps, and the second is the one that is easy to leave out. `O_CREAT`'s mode is **masked
/// by the process umask** — `open(…, 0o644)` under a umask of 0077 produces 0600 — so the
/// `.mode()` below is a ceiling and not a value. The explicit `set_permissions` is what actually
/// fixes it. `cide_core::persist::create_private` needs no such follow-up precisely because it
/// asks for 0600, which no umask can make *more* permissive; asking for a looser mode is the
/// case where the difference bites.
///
/// Both are applied to the **temp file**, before the rename, because the mode travels with the
/// inode: a chmod after the rename would leave a window in which the published file has whatever
/// the umask produced, and that window is exactly when a `git status` or a CI checkout might
/// look at it.
///
/// Overriding the user's umask is deliberate and is the one place this module does not defer to
/// the environment. A umask of 0077 says "my files are private"; this file is not the user's, it
/// is the repository's — committed, pulled by teammates, and read by a build running as another
/// account in the same checkout. A 0600 config.json is a file the next person cannot read for a
/// reason nothing on their screen would explain.
#[cfg(unix)]
fn create_shared(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
    Ok(file)
}

/// Whatever the platform's default is. cide is a Linux app; this arm exists so the module still
/// compiles elsewhere and claims nothing about permissions.
#[cfg(not(unix))]
fn create_shared(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch project root, built the way `cide-core`'s tests build theirs — this workspace
    /// has no temp-dir dependency and is not gaining one.
    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cide-agents-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn put(root: &Path, text: &str) {
        std::fs::create_dir_all(cide_dir(root)).expect("cide dir");
        std::fs::write(config_path(root), text).expect("config");
    }

    /// The default that keeps an upgrade from spawning anything in somebody's repository.
    #[test]
    fn a_project_with_no_config_file_is_disabled() {
        let root = temp("no-config");
        let config = load(&root);
        assert!(!config.agents.enabled);
        assert_eq!(config, CideConfig::default());
        assert_eq!(config.agents.max_concurrent, 2);
        assert_eq!(config.agents.harness, cide_ipc::Harness::Claude);
        assert_eq!(config.agents.isolation, Isolation::Worktree);
        assert!(!config.agents.allow_dangerous_permissions);
        // The one default in this struct that is *on*. See the field's doc: the loop it closes is
        // the feature, and the key is the way out of it rather than the way into it.
        assert!(config.agents.nudge_orchestrator);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The failure mode of guessing `true` is unattended processes in somebody's repository, so
    /// a file cide cannot read means off — never "assume the rest of it said yes".
    #[test]
    fn a_config_that_cannot_be_parsed_is_disabled() {
        let root = temp("bad-config");
        for text in [
            "{ \"agents\": { \"enabled\": true, ",
            "not json at all",
            "",
            "{ \"agents\": { \"enabled\": \"yes\" } }",
            "[]",
        ] {
            put(&root, text);
            let config = load(&root);
            assert!(!config.agents.enabled, "enabled by `{text}`");
            assert_eq!(config, CideConfig::default(), "partially applied `{text}`");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The disk shape, including the two fields the wire has no room for.
    #[test]
    fn a_full_config_reads_back_field_for_field() {
        let root = temp("full-config");
        put(
            &root,
            r#"{ "version": 1,
                "agents": { "enabled": true, "maxConcurrent": 4, "harness": "opencode",
                            "isolation": "shared", "allowDangerousPermissions": true,
                            "nudgeOrchestrator": false } }"#,
        );
        let config = load(&root);
        assert!(config.agents.enabled);
        assert_eq!(config.agents.max_concurrent, 4);
        assert_eq!(config.agents.harness, cide_ipc::Harness::Opencode);
        assert_eq!(config.agents.isolation, Isolation::Shared);
        assert!(config.agents.allow_dangerous_permissions);
        assert!(!config.agents.nudge_orchestrator);

        // The wire shape is the three fields the panel draws, and no more.
        let wire = config.agents.to_wire();
        assert!(wire.enabled);
        assert_eq!(wire.max_concurrent, 4);
        assert_eq!(wire.harness, cide_ipc::Harness::Opencode);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file written by an older cide, or by hand with only the key somebody cared about, still
    /// loads — `serde(default)` fills the rest. That is what keeps a `git pull` from breaking a
    /// checkout.
    #[test]
    fn a_partial_config_takes_the_defaults_for_the_rest() {
        let root = temp("partial-config");
        put(&root, r#"{ "agents": { "enabled": true } }"#);
        let config = load(&root);
        assert!(config.agents.enabled);
        assert_eq!(config.agents.max_concurrent, 2);
        assert_eq!(config.agents.isolation, Isolation::Worktree);
        assert!(config.agents.nudge_orchestrator);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The enable gesture: a file lands in the user's repository, so it has to be one they would
    /// be content to see in a diff.
    #[test]
    fn the_enable_gesture_writes_a_reviewable_file() {
        let root = temp("write-config");
        let mut config = CideConfig::default();
        config.agents.enabled = true;
        write(&root, &config).expect("write");

        let text = std::fs::read_to_string(config_path(&root)).expect("read back");
        assert!(
            text.contains("\n  \"agents\": {"),
            "pretty-printed:\n{text}"
        );
        assert!(text.contains("\"maxConcurrent\": 2"), "camelCase:\n{text}");
        assert!(
            text.ends_with("}\n"),
            "a trailing newline, or every diff says so"
        );
        assert!(
            !text.contains("//"),
            "JSON has no comments; the key names are the documentation"
        );
        assert_eq!(load(&root), config, "what was written is what is read");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(config_path(&root))
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            // 0644 and not `persist::write_atomic`'s 0600: this file is committed and read by
            // the team, and there is no proxy password anywhere near it.
            assert_eq!(
                mode, 0o644,
                "a committed file the team has to be able to read"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Milestones are their own key with their own reader and writer. (M83) A broken milestone
    /// costs the milestones and nothing else; a save of `agents` leaves them alone; and an empty
    /// plan takes the key out of the committed file rather than leaving `{"items": []}` behind.
    #[test]
    fn milestones_are_read_and_written_beside_agents_without_touching_them() {
        let root = temp("milestones");
        put(
            &root,
            r#"{ "version": 1, "agents": { "enabled": true },
                "milestones": { "items": [ { "id": "slice", "gate": 42 } ] } }"#,
        );
        assert!(
            load(&root).agents.enabled,
            "a bad gate does not switch subagents off"
        );
        assert!(
            load_milestones(&root).is_empty(),
            "it costs the milestones instead"
        );

        let plan = cide_ipc::MilestonePlan {
            items: vec![cide_ipc::Milestone {
                id: "slice".into(),
                title: "The slice".into(),
                gate: "tools/ci/milestone.sh slice".into(),
                ..Default::default()
            }],
            verify: "tools/dev/check.sh".into(),
            ..Default::default()
        };
        write_milestones(&root, &plan).expect("write");
        assert_eq!(load_milestones(&root), plan);

        let mut config = load(&root);
        config.agents.apply(cide_ipc::OrchestrationPatch {
            max_concurrent: Some(4),
            ..Default::default()
        });
        write(&root, &config).expect("write agents");
        assert_eq!(
            load_milestones(&root),
            plan,
            "saving agents leaves milestones as they were"
        );

        write_milestones(&root, &cide_ipc::MilestonePlan::default()).expect("clear");
        // The key, not the word: the spin prompt that `write` stores beside it says "milestones"
        // itself, and a grep would be matching cide's own prose (CLAUDE.md's self-match trap).
        let doc: Value = serde_json::from_slice(&std::fs::read(config_path(&root)).expect("read"))
            .expect("json");
        assert!(doc.get("milestones").is_none(), "{doc}");
        assert!(load(&root).agents.enabled);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The file is not cide's to own outright. A key from a newer version, and a hand-edited
    /// field the panel cannot draw, both survive a save.
    #[test]
    fn writing_preserves_what_this_build_does_not_know_about() {
        let root = temp("preserve-config");
        put(
            &root,
            r#"{ "version": 1,
                "somethingElse": { "kept": true },
                "agents": { "enabled": false, "isolation": "shared", "futureKey": 7,
                            "nudgeOrchestrator": false } }"#,
        );

        let mut config = load(&root);
        assert_eq!(config.agents.isolation, Isolation::Shared);
        assert!(!config.agents.nudge_orchestrator);
        config.agents.apply(cide_ipc::OrchestrationPatch {
            enabled: Some(true),
            ..Default::default()
        });
        write(&root, &config).expect("write");

        let text = std::fs::read_to_string(config_path(&root)).expect("read back");
        assert!(text.contains("somethingElse"), "{text}");
        assert!(text.contains("futureKey"), "{text}");
        assert!(text.contains("\"enabled\": true"), "{text}");
        // A hand-edited, wire-invisible field is not reset by a round trip through the panel.
        assert_eq!(load(&root).agents.isolation, Isolation::Shared);
        // And the one whose default is *on*: a project that said no stays saying no across a
        // save, or turning subagents on from the panel would start typing into the console of a
        // team that had explicitly opted out.
        assert!(
            !load(&root).agents.nudge_orchestrator,
            "enabling orchestration re-enabled the nudge somebody had switched off"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file so broken there is nothing to preserve is replaced, and says so by being valid
    /// afterwards.
    #[test]
    fn writing_over_an_unparseable_file_replaces_it() {
        let root = temp("replace-config");
        put(&root, "{{{ not json");
        let mut config = CideConfig::default();
        config.agents.enabled = true;
        write(&root, &config).expect("write");
        assert!(load(&root).agents.enabled);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The grace has a ceiling and no floor, and both halves are deliberate. (M67)
    #[test]
    fn a_stop_grace_is_capped_but_zero_is_a_real_answer() {
        let mut config = AgentsConfig::default();
        assert_eq!(config.stop_grace(), std::time::Duration::from_secs(60));

        // Zero is policy, not nonsense: "this project forces every stop". Clamping it up to one
        // would be cide rewriting a number somebody typed on purpose — which is exactly what
        // `max_concurrent`'s `max(1)` *does* do, because a project that can never dispatch is
        // not a policy anybody holds. The asymmetry is the point; do not "fix" it.
        config.stop_grace_secs = 0;
        assert_eq!(config.stop_grace(), std::time::Duration::ZERO);

        // The ceiling is clamped rather than refused, because a config that will not load is a
        // checkout that will not open.
        config.stop_grace_secs = 6_000;
        assert_eq!(
            config.stop_grace(),
            std::time::Duration::from_secs(u64::from(MAX_STOP_GRACE_SECS))
        );
    }

    /// A `.cide/config.json` written before M79 still loads, and the new keys read as their
    /// defaults — which for `finishInNewTab` means a project upgrades *into* the feature.
    ///
    /// That is deliberate and is the same argument `nudgeOrchestrator` and `autoDispatch` make:
    /// the loop this closes is the point, and the setting is the way out. `autoSpin` is the
    /// exception in the other direction, and this asserts it: a timer that spends money while
    /// nobody is watching may not arrive with an upgrade.
    #[test]
    fn a_config_written_before_the_spinner_existed_still_loads() {
        let root = temp("pre-m79");
        std::fs::create_dir_all(cide_dir(&root)).expect("mkdir");
        std::fs::write(
            config_path(&root),
            r#"{ "version": 1, "agents": { "enabled": true, "maxConcurrent": 4 } }"#,
        )
        .expect("write");

        let agents = load(&root).agents;
        assert!(agents.enabled);
        assert_eq!(agents.max_concurrent, 4);
        assert!(
            agents.finish_in_new_tab,
            "the review tab is the way out, not the way in"
        );
        assert!(
            !agents.auto_spin,
            "an upgrade started a billed process on a timer"
        );
        assert_eq!(agents.auto_spin_after_secs, DEFAULT_SPIN_AFTER_SECS);
        // Blank on disk is the shipped prompt, decided in one place.
        assert_eq!(agents.spin_prompt(), DEFAULT_SPIN_PROMPT);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The clamp is the reader's, not the field's, so a hand-edited file keeps what it says and
    /// the timer still refuses to fire inside the nudge coalescer's settling window.
    #[test]
    fn the_dwell_is_clamped_where_it_is_read() {
        let mut config = AgentsConfig {
            auto_spin_after_secs: 5,
            ..AgentsConfig::default()
        };
        assert_eq!(config.auto_spin_after_secs, 5, "the file was rewritten");
        assert_eq!(
            config.spin_after(),
            std::time::Duration::from_secs(u64::from(MIN_SPIN_AFTER_SECS))
        );

        config.auto_spin_after_secs = u32::MAX;
        assert_eq!(
            config.spin_after(),
            std::time::Duration::from_secs(u64::from(MAX_SPIN_AFTER_SECS))
        );

        config.auto_spin_after_secs = 1_800;
        assert_eq!(config.spin_after(), std::time::Duration::from_secs(1_800));
    }

    /// **Blank means the default and never an empty prompt.**
    ///
    /// A `claude` handed a lone Enter starts a turn about nothing and bills for it. Clearing the
    /// box in Settings means *use the one cide ships*, which is the only thing clearing it could
    /// mean — and the decision lives in one place so the Settings screen and the spawn cannot
    /// disagree about what an empty string is.
    #[test]
    fn a_blank_prompt_is_the_shipped_one() {
        for blank in ["", "   ", "\t", "\n  \n"] {
            let config = AgentsConfig {
                auto_spin_prompt: blank.into(),
                ..AgentsConfig::default()
            };
            assert_eq!(config.spin_prompt(), DEFAULT_SPIN_PROMPT, "{blank:?}");
        }
        let config = AgentsConfig {
            auto_spin_prompt: "look at the board".into(),
            ..AgentsConfig::default()
        };
        assert_eq!(config.spin_prompt(), "look at the board");
    }

    /// The review template follows the same blank rule, and a patch flattens it on the way in.
    #[test]
    fn a_blank_review_prompt_is_the_shipped_one() {
        for blank in ["", "   ", "\n  \n"] {
            let config = AgentsConfig {
                review_prompt: blank.into(),
                ..AgentsConfig::default()
            };
            assert_eq!(config.review_prompt_template(), DEFAULT_REVIEW_PROMPT);
        }
        let mut config = AgentsConfig::default();
        config.apply(OrchestrationPatch {
            review_prompt: Some("check {task_id}\non\r\n{branch}".into()),
            ..Default::default()
        });
        assert_eq!(config.review_prompt, "check {task_id} on {branch}");
        assert_eq!(
            config.to_wire().review_prompt,
            "check {task_id} on {branch}"
        );
    }

    /// **A filled value is never scanned again.** A task title that says `{branch}` is text
    /// somebody wrote into a tracker, and it must reach the reviewer as that text rather than
    /// choose what the reviewer is told. Unknown names and stray braces survive as written.
    #[test]
    fn a_placeholder_in_a_value_is_not_expanded() {
        let filled = fill_review_prompt(
            "on {task} diff {branch} {nope} {unclosed",
            &[("task", "task t-1 ({branch})"), ("branch", "cide/dev-t-1")],
        );
        assert_eq!(
            filled,
            "on task t-1 ({branch}) diff cide/dev-t-1 {nope} {unclosed"
        );
        assert_eq!(
            fill_review_prompt("a\n{x}\r\nb", &[("x", "1\n2")]),
            "a 1 2 b"
        );
    }

    /// Every placeholder the default uses is one [`REVIEW_PLACEHOLDERS`] documents, so the
    /// Settings hint cannot fall behind the prompt cide actually ships.
    #[test]
    fn every_placeholder_is_filled() {
        let values: Vec<(&str, &str)> = REVIEW_PLACEHOLDERS
            .iter()
            .map(|(name, _)| (*name, "X"))
            .collect();
        let filled = fill_review_prompt(DEFAULT_REVIEW_PROMPT, &values);
        assert!(!filled.contains('{'), "an unfilled placeholder: {filled}");
    }

    /// **The stored prompt can never hold a newline**, because a newline is a second Enter.
    ///
    /// Flattened by `apply`, on the way *in*, so the file cannot hold a prompt that would
    /// misbehave and the Settings box shows exactly what will be typed. Flattening at the spawn
    /// instead would leave a config somebody read as two lines and a run that silently got one.
    #[test]
    fn a_pasted_prompt_is_flattened_before_it_reaches_the_file() {
        let mut config = AgentsConfig::default();
        config.apply(OrchestrationPatch {
            auto_spin_prompt: Some("read the board\nthen\r\nassign   the   work".into()),
            ..Default::default()
        });
        assert_eq!(
            config.auto_spin_prompt,
            "read the board then assign the work"
        );
    }

    /// The dwell is clamped on the way in as well, so what the panel draws back is what the
    /// timer will use — `maxConcurrent`'s trade.
    #[test]
    fn a_patch_cannot_write_a_dwell_the_timer_would_refuse() {
        let mut config = AgentsConfig::default();
        config.apply(OrchestrationPatch {
            auto_spin_after_secs: Some(1),
            ..Default::default()
        });
        assert_eq!(config.auto_spin_after_secs, MIN_SPIN_AFTER_SECS);
        assert_eq!(
            config.spin_after(),
            std::time::Duration::from_secs(u64::from(MIN_SPIN_AFTER_SECS))
        );
    }

    /// `None` means "leave this alone", and a nonsense number lands as a sane one rather than as
    /// an error the frontend discards.
    #[test]
    fn a_patch_touches_only_what_it_names() {
        let mut config = AgentsConfig {
            enabled: true,
            max_concurrent: 5,
            harness: cide_ipc::Harness::Opencode,
            isolation: Isolation::Shared,
            allow_dangerous_permissions: true,
            nudge_orchestrator: false,
            auto_dispatch: false,
            permission_mode: Some("manual".into()),
            skip_permissions: None,
            stop_grace_secs: 5,
            finish_in_new_tab: false,
            auto_spin: true,
            auto_spin_after_secs: 120,
            auto_spin_prompt: "have a look".into(),
            review_prompt: "review {task_id}".into(),
        };
        config.apply(cide_ipc::OrchestrationPatch::default());
        assert_eq!(config.max_concurrent, 5);
        assert!(config.enabled);

        config.apply(cide_ipc::OrchestrationPatch {
            max_concurrent: Some(0),
            ..Default::default()
        });
        assert_eq!(config.max_concurrent, 1, "0 would never dispatch anything");

        // No disk-only field is on the wire, so no patch can reach any of them.
        assert_eq!(config.isolation, Isolation::Shared);
        assert!(config.allow_dangerous_permissions);
        assert!(
            !config.nudge_orchestrator,
            "a round trip through the panel turned the orchestrator nudge back on"
        );
        assert!(
            !config.auto_dispatch,
            "a round trip through the panel turned assignment-starts-work back on"
        );
        assert_eq!(
            config.unattended(),
            Unattended::Ask,
            "a round trip through the panel changed how unattended children are permitted — \
             this is the one switch where a silent reset re-arms them"
        );
    }

    /// The default is `auto`, and the key that replaced `skipPermissions` reads the old one
    /// without letting an upgrade widen anything. (M82)
    #[test]
    fn the_unattended_mode_defaults_to_auto_and_reads_the_old_key() {
        assert_eq!(AgentsConfig::default().unattended(), Unattended::Auto);

        let with = |mode: Option<&str>, skip: Option<bool>| AgentsConfig {
            permission_mode: mode.map(str::to_string),
            skip_permissions: skip,
            ..AgentsConfig::default()
        };
        // Every file cide wrote before M82 says `true` whether or not anybody chose it, so it
        // reads as the new default. It is not read as bypass.
        assert_eq!(with(None, Some(true)).unattended(), Unattended::Auto);
        // A project that switched prompts back on keeps them.
        assert_eq!(with(None, Some(false)).unattended(), Unattended::Ask);
        // The new key outranks the old one in both directions.
        assert_eq!(
            with(Some("bypassPermissions"), Some(false)).unattended(),
            Unattended::Bypass
        );
        assert_eq!(
            with(Some("manual"), Some(true)).unattended(),
            Unattended::Ask
        );
        // A typo fails towards asking.
        assert_eq!(with(Some("Auto-ish"), None).unattended(), Unattended::Ask);
    }

    /// Neither key is written unless the file had it, so the panel's first save cannot put
    /// `"permissionMode": "auto"` beside a hand-written `"skipPermissions": false` and outrank
    /// it. (M82)
    #[test]
    fn a_save_does_not_invent_a_permission_mode() {
        let root = temp("legacy-skip");
        put(
            &root,
            r#"{ "version": 1, "agents": { "enabled": false, "skipPermissions": false } }"#,
        );
        let mut config = load(&root);
        config.agents.apply(OrchestrationPatch {
            enabled: Some(true),
            ..Default::default()
        });
        write(&root, &config).expect("write");
        let text = std::fs::read_to_string(config_path(&root)).expect("read back");
        assert!(!text.contains("permissionMode"), "{text}");
        assert_eq!(load(&root).agents.unattended(), Unattended::Ask);

        let fresh = temp("fresh-write");
        write(&fresh, &CideConfig::default()).expect("write");
        let text = std::fs::read_to_string(config_path(&fresh)).expect("read back");
        assert!(
            !text.contains("permissionMode") && !text.contains("skipPermissions"),
            "{text}"
        );
        assert_eq!(load(&fresh).agents.unattended(), Unattended::Auto);

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&fresh);
    }

    #[test]
    fn the_config_lives_where_the_rest_of_dot_cide_does() {
        let root = Path::new("/repo");
        assert_eq!(config_path(root), Path::new("/repo/.cide/config.json"));
        assert_eq!(cide_dir(root), Path::new("/repo/.cide"));
    }
}
