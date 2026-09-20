//! Running a role on the Claude Code CLI. (M18)
//!
//! # A run is an interactive pane that nobody is looking at
//!
//! Not `-p --output-format stream-json`. The only thing the one-shot lane buys is a
//! machine-readable result envelope, and cide already has every field of it from a channel it
//! built and shipped: hook frames keyed on `CIDE_SESSION`, plus the statusline frame. What an
//! interactive PTY buys has no substitute — a run headless for twenty minutes can be **opened
//! into a pane** with its whole transcript intact, because `PtySession` feeds its `vt100` mirror
//! independently of sinks; and a permission prompt is **answerable**, because there is a terminal
//! for the answer to be typed into. `-p` has neither: `cide_ipc::headless` is explicit that that
//! lane "never owns a pane and never resumes".
//!
//! So this builds the same [`SpawnSpec`] `cmd/session.rs` builds for a pane, and everything
//! downstream — the registry, the exit watcher, the shutdown ladder, the orphan arming — applies
//! with no second implementation of any of it.
//!
//! # The argv order, which is load-bearing
//!
//! `cmd/session.rs::session_spawn` and `cide_claude::headless::argv` both refuse the same wager
//! and say so; this is the third spawn site and it must not be the one that takes it.
//!
//! 1. **The user's own arguments**, from [`cide_core::claude_cli::plan_here`]. `--add-dir`,
//!    `--mcp-config`, `--allowedTools`, `--disallowedTools` and `--tools` are all variadic —
//!    `<tools...>` in `--help` — and collect every following token that does not begin with `-`.
//!    Every token cide writes below begins with one, so a user flag placed *first* can never
//!    swallow cide's; the same flag placed last would swallow whatever cide had already written.
//! 2. **`--session-id <uuid>`**, through [`cide_claude::conversation`] rather than pushed by
//!    hand. That function owns the fresh/resume/fork vocabulary, its unit tests and the
//!    `#[ignore]`d real-CLI test that caught 2.1.227 retracting a shape. A second hand-written
//!    `--session-id` here is a second thing to fix when the CLI moves again.
//! 3. **The role's system prompt**, through
//!    [`cide_core::claude_cli::fold_append_system_prompt`] and never a raw push. Measured on
//!    2.1.235: a second `--append-system-prompt` does not error, the **last one silently wins**,
//!    and every earlier one is discarded. Pushing ours would therefore delete a user's — no
//!    warning, no log line, no non-zero exit. cide's own paragraph about the task tracker
//!    ([`super::TRACKER_PREAMBLE`]) goes through that same fold immediately afterwards, which is
//!    what leaves the user's text, then the role's brief, then cide's housekeeping, in that order
//!    and inside the one flag the CLI will read.
//! 4. **`--model`, `--effort`, `--permission-mode`, `--allowedTools`** from the definition, each
//!    emitted only when the definition carried it. `--permission-mode` in particular is validated
//!    at load against the literal set in `claude --help`, so it is not re-validated here; what is
//!    refused here is *inventing* one the definition did not ask for, because the default is the
//!    safe end of the range and a run that silently gained `acceptEdits` is a run editing files
//!    nobody authorised.
//! 5. **`--mcp-config <inline json>`** naming `cide-hook mcp`, built with serde rather than
//!    `format!` so a worktree path containing a quote or a backslash cannot produce a config the
//!    CLI parses as something else. Deliberately **not** `--strict-mcp-config`: it would silently
//!    drop every MCP server the user configured in exchange for cide's one.
//! 6. **`--settings <json>`** from [`cide_claude::inline_settings`], byte-identical to a pane's,
//!    so all ten hook points report and the statusline gives live cost.
//! 7. **`-n "<role> · <task>"`** (`-n, --name <name>  Set a display name for this session`), so
//!    `/resume` and the terminal title name the work rather than a uuid.
//!
//! # The environment, and the one variable whose absence is the point
//!
//! | name | value | why |
//! | --- | --- | --- |
//! | `TERM`, `COLORTERM`, `TERM_PROGRAM`… | [`cide_core::child_env::terminal_child_env`] | the identical list a pane gets; composed there so there is one copy |
//! | `CLAUDE_CODE_*` | the user's [`cide_ipc::ClaudeSettings`] | folded inside that same function |
//! | the user's launch variables | [`cide_core::claude_cli::plan_here`] | a subagent *is* a Claude child, so it gets what Claude children get |
//! | proxy variables | [`crate::harness::RunPlan::proxy`], resolved by the caller | one resolved answer per child, `cide-core`'s rule |
//! | `CIDE_SESSION` | the run's session | the routing key every hook frame is filed under |
//! | `CIDE_HOOK_SOCK` | the hook socket | where those frames go |
//! | `CIDE_AGENT_SOCK` | the agent-RPC socket | what `cide-hook mcp` bridges to |
//! | `CIDE_RUN` | the run id | what the bridge's header line names, so the app can scope a connection to the task tools alone |
//! | `CLAUDE_CODE_SSE_PORT` | **removed** | see below |
//! | `CLAUDE_CODE_AUTO_CONNECT_IDE` | **removed** | see below |
//!
//! **The IDE integration is deliberately switched off for a run, and this is the paragraph to
//! read before switching it back on.** `openDiff` blocks the agent's turn until a human answers a
//! tab — that is a documented invariant of `cide-app/src/ide.rs::pump`, whose every early return
//! must cancel the request for exactly this reason. A headless run has no pane, so there is no
//! tab and no human, and the turn would hang until somebody noticed a row that had stopped
//! moving, burning nothing but wall-clock and holding a concurrency slot and a worktree the whole
//! time. `cide_claude::headless::scrub_env` makes precisely this argument for the one-shot lane.
//!
//! Both names are *removed* rather than merely not set, because cide can inherit them: launching
//! `./run.sh` from inside a `claude` pane hands this process a `CLAUDE_CODE_SSE_PORT` that would
//! otherwise reach every child it spawns.
//!
//! The cost is real and it is one read: a subagent gets no `getDiagnostics`, and has to run
//! `cargo check` (or the project's own build) instead. That trade — a lost read against a wedged
//! agent spending tokens on a turn nobody can unblock — is not close.
//!
//! # cwd is the worktree, and that is also the run's resume identity
//!
//! `claude` files its transcript under the directory it started in. So a run started in
//! `.cide/worktrees/developer` is resumable *from there* and nowhere else, and a run started in
//! the project root is a different conversation as far as the CLI is concerned. Worth knowing
//! before anything "tidies up" by normalising the cwd: it would silently orphan every transcript
//! the agent had accumulated.

use cide_claude::HookEvent;
use cide_core::claude_cli::Injection;
use cide_ipc::{RunState, SessionState};
use cide_pty::{Geometry as PtyGeometry, SpawnSpec};

use cide_ipc::HarnessSession;

use super::{
    ADHOC_PREAMBLE, ContinueSpec, Delivery, ESC, Harness, HarnessError, HarnessSpawn, Observation,
    RunPlan, SERVER, SessionBinding, tracker_preamble,
};

/// The Claude Code CLI as a harness. A unit struct: it holds nothing, and must not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaudeHarness;

impl Harness for ClaudeHarness {
    fn kind(&self) -> cide_ipc::Harness {
        cide_ipc::Harness::Claude
    }

    /// `mcp__<server>__<tool>` — the CLI's own namespacing, measured while [`mcp_config`] was
    /// written and the reason cide's server is called [`SERVER`].
    fn tool_name(&self, tool: &str) -> String {
        format!("mcp__{SERVER}__{tool}")
    }

    fn spawn_spec(&self, plan: &RunPlan<'_>) -> Result<HarnessSpawn, HarnessError> {
        assemble(plan, None)
    }

    /// The continuing child — `claude --resume`, into the same worktree the transcript lives
    /// under — in one of two shapes, decided by whether `session` **is** `plan.session`.
    ///
    /// It is, for a run restored from the registry's snapshot: [`SessionBinding::Caller`] means
    /// the caller chose the id, and that resume rebinds the run to its previous
    /// [`cide_ipc::SessionId`] before the fork (`ResumePoint::rebind` in the registry), so the
    /// conversation is continued under its own id and nothing about hook routing, the row or an
    /// open pane changes. The parameter was redundant by construction for as long as that was
    /// the only continuation.
    ///
    /// It is not, for a run the registry restarts on new settings while its old child is still
    /// being wound down: the run is rebound to a **fresh** id first — `AgentRegistry::respawn`'s
    /// rule, so the dying child's exit and its last hook frames belong to nobody — and the new
    /// child is `--resume <old> --fork-session --session-id <new>`, the one shape in which naming
    /// an id beside a resume is legal (`cide_claude::conversation`). The fork inherits the whole
    /// conversation and the old transcript survives untouched.
    fn respawn_spec(
        &self,
        plan: &RunPlan<'_>,
        session: &str,
    ) -> Result<HarnessSpawn, HarnessError> {
        let parent: cide_ipc::SessionId =
            session
                .trim()
                .parse()
                .map_err(|_| HarnessError::NotAConversation {
                    harness: cide_ipc::Harness::Claude,
                    id: session.to_string(),
                })?;
        assemble(plan, Some(parent))
    }
    fn deliver(&self, text: &str) -> Delivery {
        Delivery::Stdin(submit(text))
    }

    /// Esc, which is how a person at this TUI ends a turn in flight.
    ///
    /// One byte and not two: a second Esc is the history gesture, not a harder interrupt, and a
    /// pair sent together would open the composer's history over the wind-down line about to be
    /// typed. See `cide_app::agents`' stop road for the beat that has to follow it — bytes
    /// arriving in the same `read` as the Esc are read by a TUI that is still tearing its turn
    /// down, which is `type_submitted_line`'s measured failure in a new place.
    fn interrupt(&self) -> Option<Vec<u8>> {
        Some(vec![ESC])
    }

    /// `claude`, and the conversation as the [`cide_ipc::SessionId`] it already is.
    ///
    /// No `--resume` is written here. `session_spawn` hands `resume` to
    /// [`cide_claude::conversation`], the one function that owns the fresh/resume/fork
    /// vocabulary and its real-CLI test — the module header's second point, applied to the
    /// fourth spawn site. And no role brief, on purpose: the pane is a person's, and a
    /// `--append-system-prompt` from here would be the one `session_spawn` folds *last*
    /// silently winning over theirs (the header's third point).
    fn continue_spec(&self, conversation: &HarnessSession) -> Result<ContinueSpec, HarnessError> {
        if conversation.harness != cide_ipc::Harness::Claude {
            return Err(HarnessError::WrongHarness {
                plan: conversation.harness,
                harness: cide_ipc::Harness::Claude,
            });
        }
        let id: cide_ipc::SessionId =
            conversation
                .id
                .trim()
                .parse()
                .map_err(|_| HarnessError::NotAConversation {
                    harness: cide_ipc::Harness::Claude,
                    id: conversation.id.clone(),
                })?;
        Ok(ContinueSpec {
            program: crate::defs::harness_binary(cide_ipc::Harness::Claude).to_string(),
            args: Vec::new(),
            resume: Some(id),
        })
    }

    fn observe(&self, current: RunState, ob: Observation<'_>) -> Option<RunState> {
        match ob {
            // The only observation carrying ground truth about the child, so it answers from
            // any state: a `Paused` run whose frozen child was killed under the freeze, a
            // `Failed` one that turned out to have left a process behind, and above all an
            // `Idle` one — alive by definition, and an exit is the only thing that ends it.
            //
            // It is also the **only** producer of `Finished` in this file, which is the whole
            // shape of the fix here: a turn ending is `RunState::Idle`, and a `code` is written
            // when, and only when, the reaper has one to write.
            Observation::Exit(code) => {
                let next = RunState::Finished { code };
                (next != current).then_some(next)
            }
            // A claude run's identity is `SessionBinding::Caller`, so there is nothing to scrape
            // out of its output, and its state comes from hooks rather than from a TUI whose
            // shape changes between releases. `opencode` is the harness for which this arm does
            // the work.
            Observation::Line(_) => None,
            Observation::Hook(frame) => {
                let event = frame.kind()?;
                let session = session_state_of(&current)?;

                // The permission case is decided here rather than in the state machine, which
                // cannot see the payload — exactly as `cide_app::hooks::decide` does it, and
                // deliberately the same shape so the two cannot drift.
                let next = if event == HookEvent::Notification {
                    cide_claude::is_permission_request(&frame.payload)
                        .then_some(SessionState::AwaitingPermission)
                } else {
                    cide_claude::next_state(session, event)
                }?;

                let next = run_state_of(next)?;
                (next != current).then_some(next)
            }
        }
    }

    /// The aliases, not a probe — Claude Code has no command that lists models.
    ///
    /// So this is [`crate::defs::PERMISSION_MODES`]' status rather than
    /// `cide_ipc::Harness`': a copy of somebody else's vocabulary that cide is allowed to hold
    /// stale, because the text box beside it is the real answer and a full model id is typed
    /// there the day a release adds one. Refusing an unlisted value is the failure to avoid, and
    /// the form does not.
    ///
    /// `cwd` is ignored: an alias means the same thing in every directory.
    fn models(
        &self,
        _cwd: Option<&std::path::Path>,
        _llm: &cide_ipc::LlmSettings,
    ) -> Result<Vec<String>, String> {
        Ok(MODEL_ALIASES.iter().map(|m| (*m).to_string()).collect())
    }
}

/// The model aliases `--model` accepts, newest-capability first.
///
/// Ordered the way the form should offer them and not alphabetically: a menu's first row is the
/// one a person picks without reading, so it is the middle of the range rather than the top of
/// the bill.
const MODEL_ALIASES: &[&str] = &["sonnet", "opus", "haiku"];

/// Build the child, fresh (`resume: None`) or continuing the conversation `resume` names. One
/// function so the shapes differ in exactly the conversation tokens and nothing else —
/// `opencode.rs::child`'s shape, on the harness where the difference is one argument to
/// [`cide_claude::conversation`].
fn assemble(
    plan: &RunPlan<'_>,
    resume: Option<cide_ipc::SessionId>,
) -> Result<HarnessSpawn, HarnessError> {
    // `plan.harness`, the *resolved* one, not the definition's: a role a local override moved onto
    // this CLI must not be refused by it. See `RunPlan::harness`.
    if plan.harness != cide_ipc::Harness::Claude {
        return Err(HarnessError::WrongHarness {
            plan: plan.harness,
            harness: cide_ipc::Harness::Claude,
        });
    }
    // Refused before anything is built: an interactive `claude` with no opening prompt starts
    // perfectly and then does nothing for ever. See `HarnessError::NoPrompt`.
    if plan.prompt.trim().is_empty() {
        return Err(HarnessError::NoPrompt);
    }

    // The configured binary, passed through **exactly as stored** so a bare `claude` is still
    // resolved by the OS at this spawn rather than pinned to whatever `which` answered at
    // launch — the CLI updates itself underneath a running app. Not `resolve`d here: that is
    // a `stat` per `PATH` entry, `crate::defs::installed` already asked the question at load,
    // and the sentence it produced is on the role's `unavailable` where the panel is drawing
    // it. A second wording of the same refusal would be a second thing to keep true.
    let program = plan.claude.cli.binary.trim();
    if program.is_empty() {
        return Err(HarnessError::NoBinary);
    }

    // The user's launch configuration, filtered. Enforced here as well as on the Settings
    // screen and not instead of it: `workspace.json` is hand-editable and `settings_set` is
    // one `invoke` away from being bypassed.
    //
    // Note there is no `is_claude` question to ask. A pane has to decide, because a pane may
    // be a shell; a run on this harness is a `claude` by construction, which is the whole
    // difference between the two spawn sites' shapes.
    let cli = cide_core::claude_cli::plan_here(&plan.claude.cli);
    // One line, once, naming what was dropped. There is no screen involved in a dispatch at
    // all, so the log is the only place this refusal can be seen.
    for refused in cli.refusals() {
        tracing::warn!(
            token = %refused.text,
            "refusing a claude launch argument or variable for an agent run: {}",
            refused.verdict.note().unwrap_or_default()
        );
    }

    // ---- the one question steps 3 and 5 both answer ----
    //
    // Whether this run gets the task tracker's tools at all. Computed once, here, because two
    // places downstream depend on it and they must not be able to disagree: step 5 attaches
    // `cide-hook mcp`, and step 3 tells the role to use the tools that server serves. A
    // paragraph naming a vocabulary the session does not have is a run that tries, fails, and
    // has nothing to say about why — the same argument `cmd/session.rs` makes at its roster
    // paragraph, which is gated on the same question one lane over.
    //
    // `plan.hook_bin` is that question and the whole of it: the bridge is what carries every
    // `cide_task_*` call, and the JSON below is the only other way this can come out empty.
    //
    // Deliberately **not** `cli.inject.has(Injection::McpConfig)`, although step 6 does ask
    // that question about `--settings`. Step 5 writes `--mcp-config` from `hook_bin` alone, so
    // in this lane the switch is not what decides whether the tools exist — and gating the
    // prose on a condition that does not gate the tools would silently take the paragraph away
    // from runs that have them. The two read one value so that they cannot answer differently;
    // if step 5 ever starts consulting the switch, this reads it through the same expression.
    let tracker = match &plan.hook_bin {
        Some(hook) => match mcp_config(&hook.to_string_lossy()) {
            Some(json) => Some(json),
            // Serialising a two-key JSON object cannot realistically fail; skipping the flag
            // rather than failing the spawn is still the right shape, because a run with no
            // task tools is a run that works and reports, and a refused dispatch is not.
            None => {
                tracing::warn!("cannot build an mcp config; this run gets no task tools");
                None
            }
        },
        None => None,
    };

    // ---- 1. the user's own arguments, before every token cide adds ----
    let mut args = cli.args.clone();

    // ---- 2. `--session-id <uuid>` ----
    //
    // Fresh, a plain resume, or a fork — `cide_claude::conversation`'s three shapes, chosen by
    // `resume` alone. A fork is asked for exactly when the conversation being continued is not
    // filed under this plan's session: the registry rebound the run to a fresh id before this
    // fork (see `respawn_spec` on the harness), and `--resume <parent> --session-id <new>`
    // without `--fork-session` is the pair the CLI rejects.
    //
    // `cli.inject` and not a fresh resolution, so the flag folded here is the same value the
    // refusal verdicts above were computed against: cide can never refuse a user's
    // `--session-id` while passing none of its own.
    let fork = resume.is_some_and(|parent| parent != plan.session);
    let (effective, conversation) =
        cide_claude::conversation(plan.session, resume, fork, &cli.inject);
    if effective != plan.session {
        // The degraded fork: `--fork-session` is switched off in the user's injection settings,
        // so the child continues under the *parent's* id while the registry filed it under
        // `plan.session`. Hook frames still arrive — they are routed on `CIDE_SESSION`, set
        // below to the plan's id — but the transcript lands under the parent, so the next
        // resume of `plan.session` will find nothing. Said once in the log, because there is no
        // screen involved in a dispatch and no cheaper shape to fall back to.
        tracing::warn!(
            session = %plan.session,
            parent = %effective,
            "continuing a run without --fork-session: its transcript stays under the parent id"
        );
    }
    args.extend(conversation);

    // **Is this role a Claude Code subagent?** (M30) One question, asked once, because four
    // decisions below turn on it and a second `is_claude_code()` written out at any of them is a
    // second place for the answer to drift.
    //
    // Under `--agent <name>` the CLI reads the whole definition itself — the system prompt (which
    // it *replaces* rather than appends), `model`, `tools`, `permissionMode`, and the four things
    // cide has no vocabulary for at all: `skills`, `hooks`, `mcpServers`, `maxTurns`. Writing
    // cide's own flags alongside would be cide re-deriving a subset of somebody else's semantics
    // and getting the other half silently wrong, which is exactly what choosing `--agent`
    // avoided.
    //
    // Discovery is the reason this works with no materialising step: `--agent` resolves a project
    // subagent by walking **up** from the child's working directory, and a run's cwd is
    // `<root>/.cide/worktrees/<role>-<task>` — inside the project root — so the walk reaches
    // `<root>/.claude/agents/` on its own. `~/.claude/agents/` is always visible.
    // `a_worktree_is_inside_the_project_root_so_agent_discovery_reaches_it` is what stops a later
    // change to `WORKTREES_DIR` breaking that quietly. (A *committed* `.claude/agents/` also
    // appears inside the worktree at the revision the run's branch points at, and being closer it
    // wins; same content, worth knowing when a stale one confuses somebody.)
    let subagent = plan.agent.def.scope.is_claude_code();

    // ---- 3. the role's system prompt ----
    //
    // Folded, never pushed: a second `--append-system-prompt` silently deletes the first, so a
    // raw push would take the user's prompt away with nothing on screen saying so. The fold is
    // a no-op when the role has no prompt, which cannot happen — `defs` marks an empty one
    // `unavailable` — but the function is the one that decides that, not this line.
    //
    // **Skipped for a subagent.** `--agent` makes the definition's body the child's system
    // prompt outright, so folding a copy of the same text in would say the whole brief twice —
    // and the second copy would arrive *appended to* the prompt it duplicates, where the first
    // arrived as the prompt itself.
    if !subagent {
        cide_core::claude_cli::fold_append_system_prompt(&mut args, &plan.agent.def.system_prompt);
    }

    // ---- 3b. and then cide's own paragraph about the task tracker ----
    //
    // The same fold, called a second time, and that is exactly its semantics rather than a
    // reuse of a function meant for something else: the fold joins *whatever the argv already
    // carries* with cide's addition, user's text first and cide's after it, separated by a
    // blank line. So the first call leaves `<user>\n\n<role>` and this one leaves
    // `<user>\n\n<role>\n\n<cide>` — the role's brief verbatim and ahead of cide's
    // housekeeping, in exactly one `--append-system-prompt`, in the position the user put
    // theirs. Composing the string by hand and folding once would produce the same argv and
    // would have to re-derive the `=`-spelling and last-wins rules that function measured.
    //
    // Gated, so a run with no bridge is never told to call tools it does not have. See
    // `tracker` above and `TRACKER_PREAMBLE`'s own header.
    if tracker.is_some() {
        cide_core::claude_cli::fold_append_system_prompt(
            &mut args,
            &tracker_preamble(&ClaudeHarness),
        );
        // And, for a run working an OpenSpec change, one paragraph more. (M28) Gated on the same
        // `tracker`, because it names `mcp__cide__cide_task_update` — a run with no bridge must
        // not be told to call a tool it does not have.
        if let Some(change) = plan.change.as_deref() {
            cide_core::claude_cli::fold_append_system_prompt(
                &mut args,
                &super::spec_preamble(
                    change,
                    plan.spec_cli.as_deref(),
                    plan.spec_apply.as_deref(),
                    &ClaudeHarness,
                ),
            );
        }
        // And, for a run with no task, the paragraph that countermands the one above it. (M40)
        // Inside the same gate on purpose: `ADHOC_PREAMBLE` is a correction *to*
        // `TRACKER_PREAMBLE`'s commit-as-you-go rule, and a run told nothing about the tracker
        // has nothing to be corrected on. A `change` derives from the task (`plan_dispatch`), so
        // the two folds are never both taken; this one goes last for the opencode side's parity.
        if plan.task.is_none() {
            cide_core::claude_cli::fold_append_system_prompt(&mut args, ADHOC_PREAMBLE);
        }
    }

    // ---- 4. what the definition asked for, and nothing it did not ----
    //
    // Each of these is `Option`/empty-checked rather than defaulted. The CLI's own defaults
    // are the safe end of every one of these ranges, and a value invented here would be a
    // behaviour the role's author never wrote and cannot find in their file.
    //
    // These land *after* the user's launch arguments, which for a non-variadic option means
    // the definition wins — commander keeps the last occurrence, the same last-wins rule
    // `fold_append_system_prompt` measured for `--append-system-prompt`. That is the right
    // precedence: the role file is a statement about *this run*, and the launch configuration
    // is a default for every `claude` cide starts.
    //
    // Every one of the four is **skipped for a subagent**, because the CLI reads all four out of
    // the definition file that `--agent` names. A second copy here would not merely be redundant:
    // it would win, so a `permissionMode` a subagent's author wrote could be overridden by a
    // value cide had re-derived, and `--allowedTools` would silently drop the `disallowedTools`
    // half of a definition cide has no flag for.
    if let Some(model) = &plan.agent.def.model
        && !subagent
    {
        args.push("--model".into());
        args.push(model.clone());
    }
    // Carried verbatim and deliberately unvalidated — `LoadedAgent::effort` says why: the set
    // differs per harness and per release, and cide has no list to check it against that would
    // not be wrong within a month. A bad value is the CLI's refusal to make, loudly, in the
    // run's own transcript.
    if let Some(effort) = &plan.agent.effort
        && !subagent
    {
        args.push("--effort".into());
        args.push(effort.clone());
    }
    // Already validated against `defs::PERMISSION_MODES` at load, so it is not checked again
    // here; what matters is that the role's own word always wins — an author who wrote a
    // mode meant it, restrictive or not, and the project default below never overrides one.
    if let Some(mode) = &plan.agent.permission_mode
        && !subagent
    {
        args.push("--permission-mode".into());
        args.push(mode.clone());
    } else if plan.agent.permission_mode.is_none() && plan.skip_permissions {
        // The project's default for unattended children (`agents.skipPermissions`, on unless
        // switched off): a headless run cannot answer a prompt, and a claude run that hits
        // one parks in AwaitingPermission holding its slot and its role's only worktree
        // until a human opens its pane. The config field's doc carries the whole argument;
        // the spelling is the same validated mode a role could have written itself.
        args.push("--permission-mode".into());
        args.push(crate::defs::BYPASS_PERMISSIONS.into());
    }
    // Variadic, and safe here only because every token written after it begins with `-`. An
    // argument added below this line has to think about that.
    //
    // There is no `--disallowedTools` counterpart because a definition cannot express one:
    // the front matter has a `tools` key and no `disallowed-tools` key (`defs::KNOWN_KEYS`).
    // Emitting an empty one would be a restriction the role's author never wrote.
    if !plan.agent.tools.is_empty() && !subagent {
        args.push("--allowedTools".into());
        args.extend(plan.agent.tools.iter().cloned());
    }

    // ---- 4b. and, for a Claude Code subagent, the definition itself ----
    //
    // Non-variadic, so it disturbs nothing written after it — the rule step 4's header states
    // about `--allowedTools` and step 5's about `--mcp-config` does not apply here, and this
    // sitting between them is safe only because of that. After the user's own arguments, like
    // every other token cide adds, so a definition outranks a launch default.
    if subagent {
        args.push("--agent".into());
        args.push(plan.agent.def.id.to_string());
    }

    // ---- 5. cide's own MCP server ----
    //
    // The value from the top of this function, so the flag and the paragraph in step 3b are
    // one decision and cannot come apart.
    if let Some(json) = tracker {
        args.push("--mcp-config".into());
        args.push(json);
    }

    // ---- 6. the hook payload ----
    //
    // Gated on the same injection switch a pane's is, so a user who turned `--settings` off
    // gets the same answer in both lanes. The cost is stated at that toggle: no token
    // figures, no state machine, and therefore a run whose row never moves.
    if let (Some(hook), Some(flag)) = (&plan.hook_bin, cli.inject.flag(Injection::Settings)) {
        match settings_json(&hook.to_string_lossy(), plan.theme) {
            Some(json) => {
                args.push(flag.into());
                args.push(json);
            }
            None => tracing::warn!("cannot build inline settings; this run reports no state"),
        }
    }

    // ---- 7. a name a human can read ----
    args.push("-n".into());
    args.push(session_name(plan));

    let mut spec = SpawnSpec::new(program, plan.cwd.clone()).geometry(PtyGeometry::new(
        plan.geometry.cols,
        plan.geometry.rows,
        plan.geometry.cell_width,
        plan.geometry.cell_height,
    ));
    for arg in args {
        spec = spec.arg(arg);
    }

    // The environment, in the same order a pane's is built, because the order is what makes
    // "cide sets this list" a sentence about cide rather than about one spawn site.
    //
    // `CARGO_PKG_VERSION` is this crate's, which *is* the application's: every workspace
    // member takes `version.workspace = true`, so there is one number. `TERM_PROGRAM_VERSION`
    // therefore keeps reporting what a pane reports.
    spec = spec.apply(cide_core::child_env::terminal_child_env(
        &plan.claude,
        env!("CARGO_PKG_VERSION"),
        cli.env,
    ));
    spec = spec.apply(plan.proxy.changes().to_vec());

    // The IDE integration, switched off. See the module header — this is the run-hangs-for-ever
    // case, not a tidy-up.
    spec = spec
        .env_remove("CLAUDE_CODE_SSE_PORT")
        .env_remove("CLAUDE_CODE_AUTO_CONNECT_IDE");

    // The routing key. Everything about a run's liveness — the state machine, the panel's
    // phase dot, the statusline's cost figure — is free because `cide-hook` reads this out of
    // the child's own environment and echoes it back, so nothing depends on cide and the CLI
    // agreeing about a uuid.
    spec = spec.env("CIDE_SESSION", plan.session.to_string());
    spec = spec.env("CIDE_RUN", plan.run.to_string());
    if let Some(sock) = &plan.hook_sock {
        spec = spec.env("CIDE_HOOK_SOCK", sock.to_string_lossy().to_string());
    }
    // Set even when `hook_bin` was missing and no `--mcp-config` was written, for the reason
    // `cmd/session.rs` gives one scope up: it costs a harness that ignores it nothing, and a
    // wrapper that ends up exec'ing the real `claude` with an `--mcp-config` of its own still
    // finds this socket.
    if let Some(sock) = &plan.agent_sock {
        spec = spec.env("CIDE_AGENT_SOCK", sock.to_string_lossy().to_string());
    }

    Ok(HarnessSpawn {
        spec,
        // Written into the terminal rather than passed as a positional argument: a bare prompt
        // does not begin with `-`, so `--mcp-config <configs...>` would swallow it into an MCP
        // configuration list, with no error from anything. It also means a run's first turn
        // and its fifth arrive by one code path — `deliver` below builds the same bytes.
        opening: Some(submit(&plan.prompt)),
        binding: SessionBinding::Caller,
        // Hooks are this harness's channel; the event file is another harness's.
        events: None,
    })
}

/// What a run's current state looks like to [`cide_claude::next_state`], or `None` when no
/// observation may move it.
///
/// The two `None` arms are the contract [`Harness::observe`] documents:
///
/// * **`Paused`** — the child is `SIGSTOP`ped. A frame that was already in flight when the signal
///   landed must not thaw the row; only a resume clears a freeze cide asserted.
/// * **`Finished` / `Failed`** — terminal. A late `PostToolUse` from a `cide-hook` that was still
///   writing when the child died would otherwise take a finished row back to `Running`, which is
///   the same ordering hazard `cide_app::hooks`' single-thread serialiser exists to prevent, one
///   layer up.
///
/// **[`RunState::Idle`] is emphatically not one of them.** An idle run's child is alive and the
/// whole reason the state exists is that it can be given another turn, so the `UserPromptSubmit`
/// raised by a delivered follow-up has to take it back to `Running`. A `None` here would pin the
/// row at `Idle` for the rest of the process's life — the mirror image of the bug being fixed,
/// with the run stuck instead of the code invented.
///
/// It maps to `SessionState::Idle` rather than `SessionState::AwaitingInput`, and the choice is
/// observationally inert: the two differ only in [`cide_claude::next_state`]'s `Stop` arm, which
/// tests `current == Busy` and so answers `Idle` from either — and [`run_state_of`] sends both
/// back to `RunState::Idle` anyway. `Idle` is picked because it is the *weaker* claim.
/// `AwaitingInput` asserts the conversation was handed to somebody, and this inverse cannot know
/// that: `RunState::Idle` collapses both of `next_state`'s idle states, so re-deriving the
/// stronger one would be inventing a fact the run may never have had.
///
/// `Queued` maps to `Spawning` for completeness rather than because it happens: a queued run has
/// no child, so no frame can name it.
fn session_state_of(state: &RunState) -> Option<SessionState> {
    match state {
        RunState::Queued | RunState::Starting => Some(SessionState::Spawning),
        RunState::Running => Some(SessionState::Busy),
        RunState::Idle => Some(SessionState::Idle),
        RunState::AwaitingPermission => Some(SessionState::AwaitingPermission),
        // No child at all — restored from the registry's snapshot — so no session state can
        // describe it, exactly as the terminal pair below.
        RunState::Interrupted => None,
        RunState::Paused { .. } | RunState::Finished { .. } | RunState::Failed { .. } => None,
    }
}

/// What a session state means for the run wrapped around it.
///
/// # Why both of the CLI's idle states are one run state
///
/// It is the one mapping here that is not a rename, so it is the one to justify.
/// [`cide_claude::next_state`] has two ways of not working: `AwaitingInput`, the CLI ending a
/// turn and handing the conversation back, and `Idle`, a `Stop` with no work behind it ("a
/// session whose first observed hook is a `Stop` has finished nothing"). Both mean the same
/// thing to a run — **the turn is over and the child is alive** — because a run has no human at
/// its keyboard for the handover to be *to*. That is [`RunState::Idle`], whose doc carries the
/// argument in full.
///
/// # What this deliberately no longer does
///
/// `AwaitingInput` used to map to `Finished { code: 0 }`, and the shape of that mistake is worth
/// naming so it is not reintroduced: it wrote an **exit status for a process that had not
/// exited**, so every later reader of `code` was reading a number no child produced, and one
/// indistinguishable from a clean exit.
///
/// The motive was sound and is preserved. A run left `Running` at an idle prompt holds its
/// concurrency slot and its role's only worktree until the child dies, and the queue stalls with
/// nothing on screen to explain it. What the old mapping got wrong was conflating "this run has
/// stopped working" with "this child is gone" — and only the second of those licenses a `code`.
/// `RunState::Idle` keeps the first and drops the claim; [`Observation::Exit`] is the sole
/// producer of `Finished` here, so a code appears exactly when the reaper has one.
///
/// # The one window this opens, named so the queue can close it
///
/// `SessionStart` reaches `SessionState::Idle`, so a run is briefly `Idle` between its child
/// reporting in and its opening prompt being processed — a gap of milliseconds, since those bytes
/// are typed at spawn, but a real one. A slot accounting that is *recomputed* from the phase on
/// every observation would therefore see a just-dispatched run holding nothing, and could start a
/// second run of the same role into the same worktree. The slot is held from dispatch and
/// released when a run *leaves* the working set, which is a fact about the run's history rather
/// than about its current phase; the registry owns that and this function cannot.
fn run_state_of(state: SessionState) -> Option<RunState> {
    match state {
        SessionState::Spawning | SessionState::Splash => Some(RunState::Starting),
        SessionState::Busy => Some(RunState::Running),
        SessionState::Idle | SessionState::AwaitingInput => Some(RunState::Idle),
        SessionState::AwaitingPermission => Some(RunState::AwaitingPermission),
        SessionState::Exited { code } => Some(RunState::Finished { code }),
        // **Unreachable from the one caller, and `Option` rather than a guess because of it.**
        // A pause is a fact cide asserts with `SIGSTOP`; nothing in a hook frame reports one.
        // `cide_claude::next_state` cannot produce this variant and neither can the permission
        // check beside it, so the only value that ever reaches here comes from those two.
        //
        // The alternatives were both lies. `RunState::Paused { since_unix_ms: 0 }` would mint a
        // timestamp no clock produced — the exact class of invented number `RunState::Idle`'s doc
        // was written to get rid of — and folding it into `Starting` would answer a question with
        // a different question's answer. `None` says what is true: *this observation says nothing
        // about the run*, which is `Harness::observe`'s own vocabulary and is symmetric with
        // [`session_state_of`], whose `Paused` arm answers `None` for the mirror-image reason.
        SessionState::Paused => None,
    }
}

/// One turn's worth of text, as the bytes a terminal would have produced.
///
/// `\r` and not `\n`: *"a terminal sends `\r` for Enter"* (`ui/src/terminal/keys.ts`), and the
/// CLI's TUI reads its input raw. A `\n` would leave the text sitting in the prompt unsubmitted,
/// which is the failure that looks exactly like a model taking a long time to answer.
fn submit(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(b'\r');
    bytes
}

/// The `-n` display name: the role, then the task it was dispatched for.
///
/// Built from [`RunPlan::task_title`] rather than from a lookup, so the name a transcript carries
/// stays true after the task has been renamed — `AgentRun::agent_label`'s argument, applied to the
/// other half of the same string. An ad-hoc run with no task is just the role.
fn session_name(plan: &RunPlan<'_>) -> String {
    match plan
        .task_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(title) => format!("{} · {}", plan.agent.def.label, title),
        None => plan.agent.def.label.clone(),
    }
}

/// The inline `--mcp-config` JSON attaching cide's own MCP server.
///
/// # An inline string, not a file
///
/// `claude --help`: *"`--mcp-config <configs...>`  Load MCP servers from JSON files or strings"*.
/// So nothing is written into the user's project — which matters more here than it does for a
/// pane, because a run's cwd is a git worktree that cide created and will eventually prune.
///
/// # The name is what the model sees
///
/// The CLI namespaces a server's tools as `mcp__<server>__<tool>`, so calling this server `cide`
/// is what makes the vocabulary arrive as `mcp__cide__cide_task_list`. That is the spelling for an
/// `--allowedTools` line in a role definition; the bare `cide_task_list` never appears on the
/// model's side of the wire.
///
/// # `--strict-mcp-config` is deliberately absent
///
/// It would drop every MCP server the *user* configured, silently, in exchange for cide's one.
/// Attaching a task tracker is not a reason to take somebody's own tooling away from a run.
fn mcp_config(hook_bin: &str) -> Option<String> {
    // serde, not `format!`: a worktree path under a directory with an apostrophe or a backslash in
    // its name would otherwise produce a document the CLI parses as something else — and the
    // failure surfaces as `CONNECTION_CLOSED` from a process three levels down.
    // `SERVER`, not a literal: this spelled `"cide"` by hand while `opencode.rs` held a const
    // whose doc claimed the two were "the same string on purpose". They were, by luck.
    serde_json::to_string(&serde_json::json!({
        "mcpServers": {
            SERVER: {
                "command": hook_bin,
                // `cide-hook mcp` is the bridge: stdio in, `$CIDE_AGENT_SOCK` out, and no
                // knowledge of the vocabulary at all. See its module doc.
                "args": ["mcp"],
            }
        }
    }))
    .ok()
}

/// The inline `--settings` payload: all ten hook points, plus the statusline.
///
/// [`cide_claude::StatusLine::Ours`] rather than `None`, although no status bar is showing this
/// run: the statusline frame is one of the two channels a run reports through — it is where a
/// run's live cost comes from — and `emit::session_status` forwards it whole.
fn settings_json(hook_bin: &str, theme: cide_ipc::Theme) -> Option<String> {
    let settings = cide_claude::inline_settings(
        hook_bin,
        &cide_claude::StatusLine::Ours,
        match theme {
            cide_ipc::Theme::Light => cide_claude::ClaudeTheme::Light,
            cide_ipc::Theme::Dark => cide_claude::ClaudeTheme::Dark,
        },
    );
    serde_json::to_string(&settings).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tracker paragraph as *this* harness renders it.
    ///
    /// Still derived from the one definition rather than quoted — the rule the tests below were
    /// written to hold — but through `tracker_preamble`, because the constant is now a template
    /// and the tool names in it are Claude Code's only after this harness has filled them in.
    fn tracker() -> String {
        tracker_preamble(&ClaudeHarness)
    }

    use crate::defs::LoadedAgent;
    use cide_claude::HookFrame;
    use cide_ipc::{AgentDef, AgentId, Geometry, ProjectId, RunId, SessionId, TaskId, Theme};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    /// The role every test starts from: enough to spawn, nothing optional set.
    fn role() -> LoadedAgent {
        LoadedAgent {
            def: AgentDef {
                id: AgentId("developer".into()),
                label: "Developer".into(),
                scope: cide_ipc::agents::AgentScope::Project,
                harness: cide_ipc::Harness::Claude,
                description: "Implements one task end to end.".into(),
                system_prompt: "You are the developer agent. Finish the task.".into(),
                model: None,
                unavailable: None,
                max_concurrent: 1,
                worktree: true,
            },
            origin: PathBuf::from("/repo/.cide/agents/developer.md"),
            shadows: None,
            tools: Vec::new(),
            permission_mode: None,
            effort: None,
            extras: Vec::new(),
        }
    }

    fn plan_for<'a>(agent: &'a LoadedAgent, session: SessionId) -> RunPlan<'a> {
        RunPlan {
            run: RunId::new(),
            session,
            agent,
            cwd: PathBuf::from("/repo/.cide/worktrees/developer"),
            project: ProjectId::new(),
            task: Some(TaskId("t-14".into())),
            task_title: Some("Teach the parser about tabs".into()),
            change: None,
            spec_cli: None,
            spec_apply: None,
            prompt: "Do the task described above.".into(),
            hook_bin: Some(PathBuf::from("/opt/cide/cide-hook")),
            hook_sock: Some(PathBuf::from("/run/user/1000/cide-hooks-42.sock")),
            agent_sock: Some(PathBuf::from("/run/user/1000/cide-agents-42.sock")),
            events_path: None,
            theme: Theme::Dark,
            proxy: cide_core::proxy::ProxyEnv::default(),
            geometry: Geometry::default(),
            claude: cide_ipc::ClaudeSettings::default(),
            llm: cide_ipc::LlmSettings::default(),
            choice: None,
            harness: agent.def.harness,
            // Off in the fixture, so every argv assertion below is about what the role
            // and the plan actually said; the skip default has tests of its own.
            skip_permissions: false,
        }
    }

    fn spawn(plan: &RunPlan<'_>) -> HarnessSpawn {
        ClaudeHarness.spawn_spec(plan).expect("a spawnable plan")
    }

    /// The index of a token in an argv, for the ordering assertions.
    fn at(args: &[String], token: &str) -> usize {
        args.iter()
            .position(|a| a == token)
            .unwrap_or_else(|| panic!("no {token} in {args:?}"))
    }

    /// The value following a flag.
    fn value_of<'a>(args: &'a [String], flag: &str) -> &'a str {
        &args[at(args, flag) + 1]
    }

    fn env_value<'a>(spec: &'a SpawnSpec, name: &str) -> Option<&'a str> {
        // Last wins, matching what the child actually gets.
        spec.env
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The wager `session_spawn` and `headless::argv` both refuse, refused a third time.
    ///
    /// `--add-dir` is variadic: placed after cide's tokens it would swallow the next bare word it
    /// found. Every token cide writes begins with `-`, so the only safe place for the user's is
    /// first — and this asserts the whole ordering rather than the one flag, because the property
    /// is about the boundary and not about `--add-dir`.
    #[test]
    fn the_users_own_arguments_precede_every_token_cide_adds() {
        let agent = role();
        let session = SessionId::new();
        let mut plan = plan_for(&agent, session);
        plan.claude.cli.args = vec!["--add-dir".into(), "/srv/shared".into()];

        let args = spawn(&plan).spec.args;
        assert_eq!(args[0], "--add-dir", "{args:?}");
        assert_eq!(args[1], "/srv/shared", "{args:?}");

        let last_user = 1;
        for ours in [
            "--session-id",
            "--append-system-prompt",
            "--mcp-config",
            "--settings",
            "-n",
        ] {
            assert!(
                at(&args, ours) > last_user,
                "{ours} must come after the user's arguments: {args:?}"
            );
        }
    }

    /// The measured failure `fold_append_system_prompt` exists for: on 2.1.235 a second
    /// occurrence silently wins and the first is discarded, so a raw push would delete the user's
    /// prompt with nothing anywhere saying so.
    #[test]
    fn a_users_append_system_prompt_is_folded_rather_than_dropped() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.claude.cli.args = vec!["--append-system-prompt".into(), "Always be terse.".into()];

        let args = spawn(&plan).spec.args;
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1,
            "exactly one reaches the child, or the CLI throws one away: {args:?}"
        );
        let folded = value_of(&args, "--append-system-prompt");
        assert!(folded.contains("Always be terse."), "{folded}");
        assert!(folded.contains("You are the developer agent."), "{folded}");
    }

    /// The role's brief first and byte for byte, cide's paragraph after it, one flag for both.
    ///
    /// A system prompt is a document a human wrote, so the round trip is asserted on one carrying
    /// a quote, a backslash and a newline rather than on a tidy sentence. The expected value is
    /// built from [`TRACKER_PREAMBLE`] itself and never from a quoted copy: a second definition of
    /// the paragraph — the drift this constant exists to prevent — cannot pass this assertion.
    #[test]
    fn the_roles_prompt_comes_first_and_cides_paragraph_follows_it_once() {
        let mut agent = role();
        let prompt = "Say \"hello\".\nUse C:\\tools\\bin, never \\n as a literal.\nReport back.";
        agent.def.system_prompt = prompt.into();
        let plan = plan_for(&agent, SessionId::new());

        let args = spawn(&plan).spec.args;
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1,
            "one flag carries both, or the CLI throws one away: {args:?}"
        );

        let folded = value_of(&args, "--append-system-prompt");
        assert_eq!(folded, format!("{prompt}\n\n{}", tracker()), "{folded}");
        assert_eq!(
            folded.matches(&tracker()).count(),
            1,
            "said once, not once per fold: {folded}"
        );
    }

    /// Three authors, one flag, and the order is the claim.
    ///
    /// The user's launch configuration is a default for every `claude` cide starts, the role file
    /// is the brief for *this* run, and cide's paragraph is housekeeping about the tracker. A
    /// reader of the folded value gets them in exactly that order, which is what the second call
    /// to `fold_append_system_prompt` buys — it joins onto whatever is already there rather than
    /// replacing it.
    #[test]
    fn the_users_prompt_the_roles_and_cides_arrive_in_that_order() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.claude.cli.args = vec!["--append-system-prompt".into(), "Always be terse.".into()];

        let args = spawn(&plan).spec.args;
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1,
            "{args:?}"
        );

        let folded = value_of(&args, "--append-system-prompt");
        let theirs = folded.find("Always be terse.").expect("the user's");
        let brief = folded
            .find("You are the developer agent.")
            .expect("the role's");
        let ours = folded.find(&tracker()).expect("cide's");
        assert!(theirs < brief && brief < ours, "{folded}");
    }

    /// The paragraph and the tools are one decision.
    ///
    /// No `cide-hook` beside the running binary is a packaging failure rather than a choice, and
    /// it takes the whole `cide_task_*` vocabulary with it. A role told to call
    /// `mcp__cide__cide_task_comment` in a session where no such server is attached would try,
    /// fail, and have nothing to say about why — so with the flag gone the sentences go too, and
    /// the role's own prompt reaches the child untouched.
    #[test]
    fn without_the_bridge_there_are_no_task_tools_and_nothing_is_said_about_them() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.hook_bin = None;

        let args = spawn(&plan).spec.args;
        assert!(!args.iter().any(|a| a == "--mcp-config"), "{args:?}");

        let folded = value_of(&args, "--append-system-prompt");
        assert_eq!(folded, agent.def.system_prompt, "{folded}");
        assert!(!folded.contains("mcp__cide__"), "{folded}");
    }

    /// The variable whose absence is the whole point: `openDiff` blocks the agent's turn on a
    /// human answering a tab, and a headless run has no pane for that answer.
    ///
    /// Removed rather than merely unset, because cide can inherit one — launching `./run.sh` from
    /// inside a `claude` pane is exactly that case.
    #[test]
    fn a_run_is_never_connected_to_the_ide_server() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let spec = spawn(&plan).spec;

        for name in ["CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_AUTO_CONNECT_IDE"] {
            assert_eq!(env_value(&spec, name), None, "{name} must never be set");
            assert!(
                spec.env_remove.iter().any(|k| k == name),
                "{name} must be removed from what this process inherited: {:?}",
                spec.env_remove
            );
        }
    }

    /// The routing key. Every hook frame this run's `cide-hook` writes carries it back as
    /// `spawned_as`, which is what makes the run's liveness free — and wrong here means a run
    /// whose row never moves.
    #[test]
    fn the_child_is_told_which_session_and_which_run_it_is() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let spec = spawn(&plan).spec;

        assert_eq!(
            env_value(&spec, "CIDE_SESSION"),
            Some(&*session.to_string())
        );
        assert_eq!(env_value(&spec, "CIDE_RUN"), Some(&*plan.run.to_string()));
        assert_eq!(
            env_value(&spec, "CIDE_HOOK_SOCK"),
            Some("/run/user/1000/cide-hooks-42.sock")
        );
        assert_eq!(
            env_value(&spec, "CIDE_AGENT_SOCK"),
            Some("/run/user/1000/cide-agents-42.sock")
        );

        // The argv agrees with the environment: the id the CLI is told to use is the id cide
        // files the run under.
        assert_eq!(value_of(&spec.args, "--session-id"), session.to_string());
    }

    /// Built with serde, so a worktree path with a quote in it cannot produce a document the CLI
    /// parses as something else — and it has to be a document the CLI can actually read.
    #[test]
    fn the_mcp_config_is_json_and_names_the_hook_binary() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;

        let config: Value =
            serde_json::from_str(value_of(&args, "--mcp-config")).expect("valid json");
        let server = &config["mcpServers"]["cide"];
        assert_eq!(server["command"], json!("/opt/cide/cide-hook"));
        assert_eq!(server["args"], json!(["mcp"]));

        // Taking the user's own MCP servers away in exchange for cide's is not a trade cide gets
        // to make on their behalf.
        assert!(!args.iter().any(|a| a == "--strict-mcp-config"), "{args:?}");
    }

    /// A quoted path is the case serde is used for, so it is the case that is asserted.
    #[test]
    fn a_hook_path_with_a_quote_in_it_still_parses() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.hook_bin = Some(PathBuf::from("/home/o\"brien/bin/cide-hook"));

        let args = spawn(&plan).spec.args;
        let config: Value =
            serde_json::from_str(value_of(&args, "--mcp-config")).expect("valid json");
        assert_eq!(
            config["mcpServers"]["cide"]["command"],
            json!("/home/o\"brien/bin/cide-hook")
        );
    }

    /// The project default (`agents.skipPermissions`) reaches the argv when the role said
    /// nothing — and the role's own word, whatever it is, always wins over it.
    #[test]
    fn the_skip_default_injects_bypass_and_the_roles_word_wins() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.skip_permissions = true;
        assert_eq!(
            value_of(&spawn(&plan).spec.args, "--permission-mode"),
            crate::defs::BYPASS_PERMISSIONS,
            "an unattended child cannot answer a prompt — the config field's doc has the measurement"
        );

        // The author's word wins — including a *restrictive* one, which an override here would
        // silently widen into exactly the behaviour their file says they did not want.
        let mut strict = role();
        strict.permission_mode = Some("plan".into());
        let mut plan = plan_for(&strict, SessionId::new());
        plan.skip_permissions = true;
        assert_eq!(
            value_of(&spawn(&plan).spec.args, "--permission-mode"),
            "plan"
        );
    }

    /// What a definition did not say, cide does not say either. The CLI's own defaults are the
    /// safe end of every one of these ranges, and a value invented here is a behaviour the role's
    /// author never wrote and cannot find in their file.
    #[test]
    fn nothing_the_definition_left_out_reaches_the_command_line() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;

        for absent in [
            "--model",
            "--effort",
            "--permission-mode",
            "--allowedTools",
            "--disallowedTools",
        ] {
            assert!(
                !args.iter().any(|a| a == absent),
                "{absent} must not appear for a definition that did not carry it: {args:?}"
            );
        }
    }

    /// And what it did say, reaches it verbatim.
    #[test]
    fn what_the_definition_asks_for_reaches_the_command_line() {
        let mut agent = role();
        agent.def.model = Some("opus".into());
        agent.effort = Some("high".into());
        agent.permission_mode = Some("acceptEdits".into());
        agent.tools = vec!["Read".into(), "Edit".into(), "Bash".into()];

        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;

        assert_eq!(value_of(&args, "--model"), "opus");
        assert_eq!(value_of(&args, "--effort"), "high");
        assert_eq!(value_of(&args, "--permission-mode"), "acceptEdits");

        // `--allowedTools` is variadic, so everything after it must begin with `-` or be one of
        // its own values. That is the ordering rule, checked rather than asserted in prose.
        let tools = at(&args, "--allowedTools");
        assert_eq!(&args[tools + 1..tools + 4], ["Read", "Edit", "Bash"]);
        assert!(
            args[tools + 4].starts_with('-'),
            "a variadic flag must be followed by an option, not a bare token: {args:?}"
        );
    }

    /// `-n` is what `/resume` and the terminal title show. A uuid there is unreadable, and a
    /// title derived from the current roster would rename itself when a task was renamed.
    #[test]
    fn the_session_is_named_for_the_role_and_the_task() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        assert_eq!(
            value_of(&spawn(&plan).spec.args, "-n"),
            "Developer · Teach the parser about tabs"
        );

        // An ad-hoc run has no task, and a trailing separator with nothing after it is worse than
        // no separator.
        plan.task = None;
        plan.task_title = None;
        assert_eq!(value_of(&spawn(&plan).spec.args, "-n"), "Developer");
    }

    /// (M40) A run with no task stands in the user's own tree, and `TRACKER_PREAMBLE` tells every
    /// run to commit as it goes — so the task-less run is told, in the same durable position and
    /// after it, that it must not. Gated with the tracker paragraph: no bridge, neither.
    #[test]
    fn an_adhoc_run_is_told_it_stands_in_the_users_tree_and_must_not_commit() {
        let agent = role();
        let mut plan = plan_for(&agent, SessionId::new());
        plan.task = None;
        plan.task_title = None;

        let args = spawn(&plan).spec.args;
        assert_eq!(
            args.iter()
                .filter(|a| *a == "--append-system-prompt")
                .count(),
            1,
            "folded, never pushed: {args:?}"
        );
        let told = value_of(&args, "--append-system-prompt");
        assert!(
            told.ends_with(&format!("{}\n\n{ADHOC_PREAMBLE}", tracker())),
            "{told}"
        );

        // A run on a task is told nothing of the sort.
        let with_task = spawn(&plan_for(&agent, SessionId::new()));
        let on_a_task = value_of(&with_task.spec.args, "--append-system-prompt");
        assert!(!on_a_task.contains(ADHOC_PREAMBLE), "{on_a_task}");

        // No bridge, no tracker paragraph — and no correction to it either.
        plan.hook_bin = None;
        let without_bridge = spawn(&plan);
        let unbridged = value_of(&without_bridge.spec.args, "--append-system-prompt");
        assert!(!unbridged.contains(&tracker()), "{unbridged}");
        assert!(!unbridged.contains(ADHOC_PREAMBLE), "{unbridged}");
    }

    /// The prompt travels in the terminal, not the argv — a bare token after
    /// `--mcp-config <configs...>` would be swallowed into an MCP configuration list with no
    /// error from anything. It is also the same bytes a follow-up produces.
    #[test]
    fn the_opening_prompt_is_typed_rather_than_passed() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let spawned = spawn(&plan);

        assert_eq!(
            spawned.opening.as_deref(),
            Some(b"Do the task described above.\r".as_slice())
        );
        assert!(
            !spawned.spec.args.iter().any(|a| a == &plan.prompt),
            "{:?}",
            spawned.spec.args
        );
        assert_eq!(spawned.binding, SessionBinding::Caller);

        // `\r`, because a terminal sends `\r` for Enter. `\n` leaves the text in the prompt
        // unsubmitted, which looks exactly like a model taking a long time.
        assert_eq!(
            ClaudeHarness.deliver("carry on"),
            Delivery::Stdin(b"carry on\r".to_vec())
        );
    }

    /// The run starts where its worktree is, and that is also where its transcript is filed — so
    /// this is the run's resume identity, not a cosmetic choice.
    #[test]
    fn a_run_starts_in_the_worktree_it_was_given() {
        let agent = role();
        let plan = plan_for(&agent, SessionId::new());
        let spec = spawn(&plan).spec;
        assert_eq!(spec.cwd, PathBuf::from("/repo/.cide/worktrees/developer"));
        assert_eq!(spec.program, "claude");
    }

    /// Three refusals that are values rather than an `ENOENT` from three processes below.
    #[test]
    fn a_plan_this_harness_cannot_run_is_refused_with_a_reason() {
        let mut agent = role();
        let session = SessionId::new();

        // `unwrap_err` rather than comparing the whole `Result`: `SpawnSpec` is not `PartialEq`
        // and giving it one so a test could say `assert_eq!(.., Err(..))` would be a trait impl
        // on a shipped type existing for a test's convenience.
        agent.def.harness = cide_ipc::Harness::Opencode;
        assert_eq!(
            ClaudeHarness
                .spawn_spec(&plan_for(&agent, session))
                .unwrap_err(),
            HarnessError::WrongHarness {
                plan: cide_ipc::Harness::Opencode,
                harness: cide_ipc::Harness::Claude,
            }
        );

        let agent = role();
        let mut plan = plan_for(&agent, session);
        plan.prompt = "   ".into();
        assert_eq!(
            ClaudeHarness.spawn_spec(&plan).unwrap_err(),
            HarnessError::NoPrompt
        );

        let mut plan = plan_for(&agent, session);
        plan.claude.cli.binary = "  ".into();
        assert_eq!(
            ClaudeHarness.spawn_spec(&plan).unwrap_err(),
            HarnessError::NoBinary
        );
        // Every refusal names its own fix; a greyed row with nothing on it is the state this
        // project has already paid for twenty-four times.
        assert!(
            HarnessError::NoBinary.to_string().contains("Settings"),
            "{}",
            HarnessError::NoBinary
        );
    }

    fn hook(event: &str) -> HookFrame {
        HookFrame::new(event, json!({ "session_id": "irrelevant" }))
    }

    /// The reuse that is the point of `observe`: zero new state-machine code, including
    /// `next_state`'s subtlest rule.
    ///
    /// A `SubagentStop` is a subagent of *this* run finishing, not the run's turn. Treating it as
    /// the end frees the concurrency slot and marks the row done in the middle of a multi-agent
    /// task — which is exactly when interrupting it costs the most.
    #[test]
    fn a_stop_idles_the_turn_and_a_subagent_stop_does_not() {
        let h = ClaudeHarness;

        // `Idle`, and the inequality is asserted as well as the equality because the failure
        // being guarded is a specific wrong answer, not any wrong answer: `Finished { code: 0 }`
        // is an exit status for a process that is still there to be given another turn.
        let stopped = h.observe(RunState::Running, Observation::Hook(&hook("Stop")));
        assert_eq!(stopped, Some(RunState::Idle));
        assert_ne!(stopped, Some(RunState::Finished { code: 0 }));

        assert_eq!(
            h.observe(RunState::Running, Observation::Hook(&hook("SubagentStop"))),
            None,
            "a subagent finishing is not the turn ending"
        );
        // The same rule from the state a multi-agent turn actually sits in: a `SubagentStop`
        // while the run is idle must not invent a transition either.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Hook(&hook("SubagentStop"))),
            None,
        );
    }

    /// An idle run is **alive**, and the two things that follow from that.
    ///
    /// This is the pair the old `AwaitingInput -> Finished { code: 0 }` mapping could not
    /// express: a follow-up can still move it, and only the reaper can end it.
    #[test]
    fn an_idle_run_resumes_and_only_an_exit_ends_it() {
        let h = ClaudeHarness;

        // What `Delivery::Stdin` produces, one hook later. A `Finished` row could not do this —
        // `session_state_of` refuses to move a terminal state, by design.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Hook(&hook("UserPromptSubmit"))),
            Some(RunState::Running),
        );
        // A second `Stop` on a run that already handed its turn back is not a transition.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Hook(&hook("Stop"))),
            None,
        );
        // And a permission request still reaches the user from here: an idle run that is asked
        // something is a run waiting on an answer, not a run that has stopped.
        let permission = HookFrame::new(
            "Notification",
            json!({ "message": "Claude needs your permission to use Bash" }),
        );
        assert_eq!(
            h.observe(RunState::Idle, Observation::Hook(&permission)),
            Some(RunState::AwaitingPermission),
        );

        // The real code, from the only observation that has one. Not zero, and not written when
        // the turn ended.
        assert_eq!(
            h.observe(RunState::Idle, Observation::Exit(3)),
            Some(RunState::Finished { code: 3 }),
        );
    }

    /// The rest of the mapping, which is a rename in every arm but the idle pair.
    #[test]
    fn hooks_drive_a_run_the_way_they_drive_a_pane() {
        let h = ClaudeHarness;

        // `SessionStart` is the child reporting in with no turn behind it yet: alive, not
        // working, which is `Idle`. The gap before the opening prompt lands is short — those
        // bytes are typed at spawn — but it is real, and `run_state_of` names what the queue
        // owes it.
        assert_eq!(
            h.observe(RunState::Starting, Observation::Hook(&hook("SessionStart"))),
            Some(RunState::Idle),
        );
        assert_eq!(
            h.observe(
                RunState::Running,
                Observation::Hook(&hook("UserPromptSubmit"))
            ),
            None,
            "already running; a transition to the state it holds is not a transition"
        );

        // The one case the state machine cannot decide, because it cannot see the payload.
        let permission = HookFrame::new(
            "Notification",
            json!({ "message": "Claude needs your permission to use Bash" }),
        );
        assert_eq!(
            h.observe(RunState::Running, Observation::Hook(&permission)),
            Some(RunState::AwaitingPermission),
        );
        let idle_nudge = HookFrame::new("Notification", json!({ "message": "Still working…" }));
        assert_eq!(
            h.observe(RunState::Running, Observation::Hook(&idle_nudge)),
            None,
        );

        // A `Stop` with no work behind it has finished nothing — `next_state`'s own rule, carried
        // through unchanged rather than re-decided here. It reaches `SessionState::Idle` rather
        // than `AwaitingInput`, and both of those are one run state, so the run is idle: alive,
        // having done nothing, waiting for the prompt that never arrived.
        assert_eq!(
            h.observe(RunState::Starting, Observation::Hook(&hook("Stop"))),
            Some(RunState::Idle),
        );

        // Output lines say nothing about a claude run: its identity is `Caller` and its state
        // comes from hooks, never from scraping a TUI whose shape moves between releases.
        assert_eq!(h.observe(RunState::Running, Observation::Line("…")), None);
    }

    /// The two states no observation may move, and the one observation that moves anything.
    #[test]
    fn a_frozen_or_finished_run_is_not_moved_by_a_late_frame() {
        let h = ClaudeHarness;

        let paused = RunState::Paused {
            since_unix_ms: 1_700_000_000_000,
        };
        assert_eq!(
            h.observe(paused.clone(), Observation::Hook(&hook("PostToolUse"))),
            None,
            "only a resume clears a freeze cide asserted"
        );

        let finished = RunState::Finished { code: 0 };
        assert_eq!(
            h.observe(finished.clone(), Observation::Hook(&hook("PostToolUse"))),
            None,
            "a late frame must not resurrect a finished run"
        );

        // Except the reaper, which is the only source of ground truth about the child — and so
        // the only one that may correct a code written when the turn ended.
        assert_eq!(
            h.observe(finished, Observation::Exit(2)),
            Some(RunState::Finished { code: 2 }),
        );
        assert_eq!(
            h.observe(paused, Observation::Exit(0)),
            Some(RunState::Finished { code: 0 }),
        );
    }

    // ======================================================================================
    // Claude Code subagents: `--agent`, and the four flags that stand down for it. (M30)
    // ======================================================================================

    /// The same role, read from `.claude/agents/` instead of `.cide/agents/`, with every switch
    /// a definition can carry set — so a flag that leaks through has something to leak.
    fn subagent() -> LoadedAgent {
        let mut agent = role();
        agent.def.scope = cide_ipc::agents::AgentScope::ClaudeProject;
        agent.def.id = AgentId("code-reviewer".into());
        agent.def.model = Some("sonnet".into());
        agent.origin = PathBuf::from("/repo/.claude/agents/code-reviewer.md");
        agent.tools = vec!["Read".into(), "Grep".into()];
        agent.permission_mode = Some("acceptEdits".into());
        agent.effort = Some("high".into());
        agent
    }

    /// **The whole of the dispatch decision, in one assertion.** `--agent` names the definition
    /// and the four flags cide would otherwise have re-derived are absent, because the CLI reads
    /// all four out of the file itself — and a second copy on the command line would *win*.
    #[test]
    fn a_subagent_is_named_rather_than_re_derived() {
        let agent = subagent();
        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;

        assert_eq!(value_of(&args, "--agent"), "code-reviewer");

        for flag in ["--model", "--effort", "--allowedTools"] {
            assert!(
                !args.iter().any(|a| a == flag),
                "{flag} is the definition's to state, not cide's: {args:?}"
            );
        }
        // The role names a mode, so cide neither passes it nor substitutes its own.
        assert!(!args.iter().any(|a| a == "--permission-mode"), "{args:?}");
    }

    /// The role's brief reaches the child as the CLI's *system prompt*, not appended to one — so
    /// cide must not fold a second copy in. What it must still fold is its own housekeeping, or
    /// the run has no idea the task tracker exists and reports to nobody.
    #[test]
    fn a_subagents_prompt_is_the_clis_to_set_but_the_tracker_paragraph_is_not() {
        let agent = subagent();
        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;

        let folded = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .map(|at| args[at + 1].clone())
            .unwrap_or_default();
        assert!(
            !folded.contains(&agent.def.system_prompt),
            "the definition's body is `--agent`'s to apply, not ours to repeat: {folded}"
        );
        assert!(
            folded.contains("cide_task_"),
            "a run nobody can hear from is a run nobody can see: {folded}"
        );
    }

    /// A subagent that names no `permissionMode` still gets cide's unattended default, for the
    /// reason it has always got it: a headless run cannot answer a prompt, and one that hits a
    /// prompt parks holding its slot and its checkout until a human opens its pane.
    #[test]
    fn a_subagent_without_a_mode_still_gets_the_unattended_default() {
        let mut agent = subagent();
        agent.permission_mode = None;
        let mut plan = plan_for(&agent, SessionId::new());
        plan.skip_permissions = true;
        let args = spawn(&plan).spec.args;
        assert_eq!(
            value_of(&args, "--permission-mode"),
            crate::defs::BYPASS_PERMISSIONS
        );
    }

    /// A cide role is untouched by any of it — the four flags are back and `--agent` is not.
    #[test]
    fn a_cide_role_is_spawned_exactly_as_before() {
        let mut agent = subagent();
        agent.def.scope = cide_ipc::agents::AgentScope::Project;
        let plan = plan_for(&agent, SessionId::new());
        let args = spawn(&plan).spec.args;
        assert!(!args.iter().any(|a| a == "--agent"), "{args:?}");
        for flag in ["--model", "--effort", "--allowedTools", "--permission-mode"] {
            assert!(args.iter().any(|a| a == flag), "{flag} missing: {args:?}");
        }
    }

    /// A continuation under the run's own id is a plain `--resume`, and nothing else — the
    /// restored-run shape, where the registry rebound the run to its previous id first.
    #[test]
    fn respawning_under_the_same_id_is_a_plain_resume() {
        let agent = role();
        let session = SessionId::new();
        let plan = plan_for(&agent, session);
        let args = ClaudeHarness
            .respawn_spec(&plan, &session.to_string())
            .expect("a uuid is a claude conversation")
            .spec
            .args;
        assert_eq!(value_of(&args, "--resume"), session.to_string());
        for absent in ["--fork-session", "--session-id"] {
            assert!(!args.iter().any(|a| a == absent), "{absent} in {args:?}");
        }
    }

    /// A continuation under a **fresh** id is a fork: `--resume <old> --fork-session
    /// --session-id <new>`. This is the shape a restart on new settings takes — the registry
    /// rebinds the run before winding the old child down, so the dying child's exit belongs to
    /// nobody — and `--resume <old> --session-id <new>` without the fork flag is exactly the pair
    /// 2.1.227 refuses (`cide_claude::session`'s header).
    #[test]
    fn respawning_under_a_fresh_id_forks_the_conversation() {
        let agent = role();
        let old = SessionId::new();
        let new = SessionId::new();
        let plan = plan_for(&agent, new);
        let args = ClaudeHarness
            .respawn_spec(&plan, &old.to_string())
            .expect("a uuid is a claude conversation")
            .spec
            .args;
        assert_eq!(value_of(&args, "--resume"), old.to_string());
        assert_eq!(value_of(&args, "--session-id"), new.to_string());
        assert!(
            at(&args, "--fork-session") > at(&args, "--resume"),
            "the fork flag qualifies the resume: {args:?}"
        );

        // Not a uuid: refused with the id in the sentence, `continue_spec`'s rule.
        let refused = ClaudeHarness
            .respawn_spec(&plan, "ses_not_a_uuid")
            .expect_err("not a claude conversation");
        assert!(refused.to_string().contains("ses_not_a_uuid"), "{refused}");
    }

    /// The real harness on a finished run's conversation is `claude` with the id handed back
    /// as a session to resume — never a `--resume` spelled here. (M42)
    #[test]
    fn continuing_a_conversation_names_claude_and_hands_the_id_back_as_a_resume() {
        use cide_ipc::{Harness, HarnessSession};

        let id = cide_ipc::SessionId::new();
        let spec = ClaudeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Claude,
                id: id.to_string(),
                cwd: std::path::PathBuf::from("/p/.cide/worktrees/developer-t-1"),
            })
            .expect("a uuid is a claude conversation");
        assert_eq!(spec.program, "claude");
        assert!(
            spec.args.is_empty(),
            "the flag is `cide_claude::conversation`'s to write"
        );
        assert_eq!(spec.resume, Some(id));

        // Not a uuid: refused with the id in the sentence, rather than handed to a CLI that
        // would fail after the pane had opened.
        let refused = ClaudeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Claude,
                id: "ses_not_a_uuid".into(),
                cwd: std::path::PathBuf::from("/p"),
            })
            .expect_err("not a claude conversation");
        assert!(refused.to_string().contains("ses_not_a_uuid"), "{refused}");

        // The other harness's conversation is the other harness's to open.
        let wrong = ClaudeHarness
            .continue_spec(&HarnessSession {
                harness: Harness::Opencode,
                id: id.to_string(),
                cwd: std::path::PathBuf::from("/p"),
            })
            .expect_err("wrong harness");
        assert!(
            matches!(wrong, HarnessError::WrongHarness { .. }),
            "{wrong}"
        );
    }
}
