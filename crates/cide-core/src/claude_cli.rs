//! What the user is allowed to add to a `claude` launch, and what happens to the rest.
//!
//! [`cide_ipc::ClaudeCli`] is the stored shape — a binary, a list of argument tokens, a list of
//! `NAME=value` pairs. This module is the *rule* over it: which of those tokens cide refuses,
//! which it merely warns about, where the survivors go in the argv, and whether the binary can
//! be executed at all.
//!
//! # Why a module in `cide-core` rather than a few `if`s at the spawn site
//!
//! Three consumers need the same answers and only one of them is a spawn.
//!
//! * `cmd::session.rs` folds the survivors into a `SpawnSpec`, once per pane.
//! * `cmd::settings.rs` answers the Settings screen, which prints the resolved argv with the
//!   refused tokens struck out. That readout is the *only* place a user finds out, because
//!   `ui/src/settings/useSettings.ts` sends its patch fire-and-forget with `.catch(() => {})` —
//!   a `settings_set` that returned `Err` today is invisible, the field snaps back and nothing
//!   is said. The proxy screen already made this argument and answered it the same way: store
//!   what was typed, and show what the child actually gets.
//! * `ui/src/settings/claudeCli.ts` is a port of the two verdict functions, so the screen can
//!   strike a token out as it is typed without a round trip. It is a port and therefore drift —
//!   `ui/scripts/check-claude-cli.mjs` reads the tables below out of *this file* and compares,
//!   so a name added on one side and not the other fails a gate rather than showing a wrong
//!   label.
//!
//! Rust remains the authority. A divergence costs a wrong label on a screen; it can never give
//! a child an argument this file refuses, because [`plan`] runs at the spawn and not in the UI.
//!
//! # Refused at the boundary *and* at the spawn, which is not belt-and-braces
//!
//! The screen's job is to make the refusal legible — a struck-out token with a sentence beside
//! it is the difference between "cide ignored my flag" and "cide told me why". The spawn's job
//! is to be true: `workspace.json` is hand-editable, `settings_set` is one `invoke` away from
//! being bypassed, and a filter that only ran in the UI would let a hand-edited file cost
//! somebody their `--resume` with no way to find out. `cmd::settings::apply_patch` makes the
//! same argument three times over for its clamps.
//!
//! # The two rules that are not negotiable
//!
//! `CLAUDE.md` states them and this module is where a free-form editor would break them:
//!
//! * **cide never injects `ANTHROPIC_API_KEY`.** It outranks subscription OAuth in the CLI's
//!   credential precedence, so a key set here would silently bill a Console organisation for a
//!   user on Claude Max — and the Settings screen carries a sentence promising cide does not do
//!   this, which a free-form editor would make into a lie. Refused by name, along with
//!   `ANTHROPIC_AUTH_TOKEN`, which is the same credential by another door.
//!
//!   The scope is exact and the inverse matters as much: this refuses cide *setting* one from
//!   settings. It never *removes* one the user's own login environment carries — `cide_claude::
//!   headless::scrub_env` records why, and it is that Console customers exist for whom it is the
//!   only credential they have. Nothing here emits a removal at all; see [`user_env`].
//!
//! * **A value that points inside the running bundle is refused**, because re-adding one is how
//!   ADR 0007's bug comes back. That is not a name list: it is [`crate::child_env::
//!   bundle_scrub_from`] run over the user's own pairs, so the rule that removes
//!   `PYTHONHOME=$APPDIR/usr` on the way out is the same rule that refuses to let it be typed
//!   back in. `LD_LIBRARY_PATH=/opt/mylib` is legitimate and passes; `LD_LIBRARY_PATH=$APPDIR/
//!   usr/lib` is the failure that is reported three processes away as `CONNECTION_CLOSED`.

use std::path::{Path, PathBuf};

use cide_ipc::{ClaudeCli, ClaudeInjection, ClaudeInjections};

use crate::child_env::EnvChange;

// ==========================================================================================
// The arguments cide adds itself, and whether it still adds them.
// ==========================================================================================

/// One argument **cide itself** puts on a Claude pane's command line.
///
/// # Why four toggles with a spelling, rather than a harness profile
///
/// The need this answers was reported as *"need to be able to rename arguments or even
/// disable, in case i want to run another harness instead of claude code, for example opencode
/// or some wrapper over the claude code"*. Three shapes could serve it; this is the one that
/// did, and the two that lost are worth keeping written down because both look cheaper.
///
/// * **A coarse harness profile** — one enum on [`cide_ipc::ClaudeCli`], `ClaudeCode` (today)
///   or `Raw` (inject nothing). Half this code and a one-row screen, and it collapses two
///   decisions whose costs are not remotely alike. A wrapper that forwards argv verbatim wants
///   everything cide injects; a wrapper that mints its own conversation ids wants the hooks and
///   not `--session-id`. One enum makes that second user give up hooks — and with them the
///   status bar's token and cost figures, the busy-versus-idle close confirm, the fast buffer
///   reload, the finished-turn notification and the CLI's own theme — in order to drop a single
///   flag. Hooks are by far the expensive half of what cide injects, and forcing them off to
///   fix an unrelated flag is a trade nobody would choose. The stored shape below would migrate
///   to a profile cleanly if the four-row screen ever proves confusing; the reverse is not true.
/// * **Toggles with no rename.** Satisfies the letter of the report, which led with "disable".
///   Rejected because once the injected set is a *table* rather than four string literals in
///   two functions, the spelling is one more column of that table and needs no new mechanism —
///   and because refusing a rename would be arbitrary beside [`cide_ipc::ClaudeCli::binary`],
///   which already lets the user substitute the *program*. If the rename inputs turn out to be
///   a footgun they can be dropped without touching the toggles.
/// * **A free-form argv template** (`--session-id {id}`). The most flexible, and rejected
///   hardest. It re-invents the quoting language `ClaudeCli::args` explicitly refused to invent
///   — one token per row, straight to `execvp`, no shell and no parser to get wrong — and,
///   decisively, it would stop the injected flag set being **enumerable**. That property is the
///   only reason the conditional half of [`REFUSED_ARGS`] can be derived from this table at
///   all: a template is text, and you cannot ask text which flags it will emit. The refusal
///   list and the injection could then only be kept in step by a human, which is exactly the
///   arrangement [`RefusedArg::because`] exists to end.
///
/// Adding a variant is one row of [`INJECTIONS`], one field on [`cide_ipc::ClaudeInjections`],
/// and one `because` on the refusal it relaxes — and the set-equality test refuses to let the
/// third be forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Injection {
    /// `--session-id <uuid>`, on a fresh session and on a fork. The uuid *is* the pane's
    /// `SessionId`, which is what makes resume free.
    SessionId,
    /// `--resume <uuid>`, when a restored pane continues its conversation.
    Resume,
    /// `--fork-session`, beside a resume, when a split branches the conversation.
    ForkSession,
    /// `--settings <inline json>`: the hook payload, and therefore everything cide knows about
    /// what a pane's agent is doing.
    Settings,
}

/// One row of [`INJECTIONS`]: what cide adds, how it spells it, and where the user's
/// configuration for it lives.
pub struct InjectionSpec {
    pub injection: Injection,
    /// The spelling cide uses when the user has not overridden it — `claude --help`'s, checked
    /// rather than remembered, exactly as [`REFUSED_ARGS`]'s are.
    pub default_flag: &'static str,
    /// The camelCase name of this injection's field on [`cide_ipc::ClaudeInjections`].
    ///
    /// What crosses the wire, and therefore what a Settings row is keyed by — a discarded
    /// rename's sentence is looked up under this, not under the flag, because the flag it was
    /// looked up by is the one that was thrown away. `check-claude-cli.mjs` asserts these are
    /// the same four names the screen uses; a typo here is a sentence that never appears.
    pub key: &'static str,
    /// Which field of [`cide_ipc::ClaudeInjections`] configures this one.
    ///
    /// A function pointer rather than a `match` at each of the four consumers: an injection
    /// added there and not here would be a stored setting nothing reads, which is this
    /// project's most-repeated defect. Returns a reference, so resolving an argv costs no
    /// allocation beyond the spellings themselves.
    pub of: fn(&ClaudeInjections) -> &ClaudeInjection,
}

/// Everything cide puts on a Claude pane's command line that the user did not type.
///
/// The list is closed and it is this one. `cide_claude::conversation` emits the first three and
/// `cmd::session.rs` the fourth; nothing else adds an argument to a pane, and the headless
/// one-shot lane (`cide_claude::headless::argv`) is deliberately not here — see its own note
/// and the sentence the Settings screen carries about it.
pub const INJECTIONS: &[InjectionSpec] = &[
    InjectionSpec {
        injection: Injection::SessionId,
        key: "sessionId",
        default_flag: "--session-id",
        of: |inject| &inject.session_id,
    },
    InjectionSpec {
        injection: Injection::Resume,
        key: "resume",
        default_flag: "--resume",
        of: |inject| &inject.resume,
    },
    InjectionSpec {
        injection: Injection::ForkSession,
        key: "forkSession",
        default_flag: "--fork-session",
        of: |inject| &inject.fork_session,
    },
    InjectionSpec {
        injection: Injection::Settings,
        key: "settings",
        default_flag: "--settings",
        of: |inject| &inject.settings,
    },
];

/// The row for one injection. Total, because [`INJECTIONS`] is exhaustive by construction and
/// a missing row would be a panic in a `const` table rather than a runtime surprise.
pub fn spec_of(which: Injection) -> &'static InjectionSpec {
    INJECTIONS
        .iter()
        .find(|spec| spec.injection == which)
        .expect("INJECTIONS covers every Injection variant")
}

/// The flags cide will actually put on this pane's command line, in [`INJECTIONS`] order.
///
/// A resolved value rather than a `&ClaudeInjections`, and that is the point: the spelling has
/// already survived the override rules by the time anything holds one of these, so the spawn,
/// the refusal table and the Settings readout cannot disagree about what will be written. It
/// travels inside [`Plan`] for the same reason `Plan` exists at all.
///
/// `Default` is **nothing injected**, which is what a shell pane gets — see `Plan::default()`'s
/// use at the spawn site. The configured default is [`Injected::defaults`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Injected(Vec<(Injection, String)>);

impl Injected {
    /// The spelling this injection will be written with, or `None` when it is switched off.
    ///
    /// The one accessor a caller should reach for: `if let Some(flag) = injected.flag(..)` is
    /// simultaneously the enabled check and the spelling, so there is no way to write the flag
    /// without having asked whether it is on.
    pub fn flag(&self, which: Injection) -> Option<&str> {
        self.0
            .iter()
            .find(|(injection, _)| *injection == which)
            .map(|(_, flag)| flag.as_str())
    }

    /// Whether cide passes this argument at all.
    pub fn has(&self, which: Injection) -> bool {
        self.flag(which).is_some()
    }

    /// Every injection, at its default spelling: byte-for-byte what cide passed before these
    /// switches existed, and what [`cide_ipc::ClaudeCli::default`] still resolves to.
    ///
    /// For tests and for `tests/real_session_args.rs`, which puts these shapes in front of the
    /// installed binary and must keep asking about the shipped default.
    pub fn defaults() -> Self {
        Self(
            INJECTIONS
                .iter()
                .map(|spec| (spec.injection, spec.default_flag.to_string()))
                .collect(),
        )
    }

    /// The pairs, in [`INJECTIONS`] order. What the set-equality test walks.
    pub fn iter(&self) -> impl Iterator<Item = (Injection, &str)> {
        self.0
            .iter()
            .map(|(injection, flag)| (*injection, flag.as_str()))
    }
}

/// An override that is not a flag, discarded. See [`injected`].
const OVERRIDE_NOT_A_FLAG: &str = "An argument cide adds has to begin with `-`. A bare token \
                                   would be claude's first POSITIONAL argument, which is a \
                                   *prompt* — every pane would start by asking the model \
                                   something. cide is using its own spelling instead.";

/// Two injections given one spelling, both discarded. See [`injected`].
const OVERRIDE_COLLIDES: &str = "Two of the arguments cide adds cannot be spelled the same, and \
                                 this one clashes with another. A command line carrying one \
                                 flag twice is the silent breakage this whole screen exists to \
                                 prevent. cide is using its own spelling instead.";

/// Resolve [`cide_ipc::ClaudeInjections`] against [`INJECTIONS`]: what cide will write, and a
/// note per override it threw away.
///
/// # The two rules an override has to survive
///
/// * **It must begin with `-`.** `claude`'s first positional argument is a prompt — the same
///   trap [`user_args`]'s value-swallowing rule exists for — so an override of `sid` rather
///   than `--sid` would not be a differently-named flag, it would start every pane by asking
///   the model something.
/// * **It must not be another injection's spelling**, its default included, and a clash
///   discards *both* sides rather than picking a winner. Two injections writing one flag is
///   the duplicate-flag bug this module is about, arriving through the new door; and a rule
///   that silently preferred whichever row came first in the table would make the outcome
///   depend on an ordering nobody can see. Reserved even when the other injection is switched
///   off, because a rename that becomes invalid the moment an unrelated toggle is flipped back
///   on is worse than one that was never accepted.
///
/// A discarded override falls back to the default spelling, which is always safe: the defaults
/// are distinct from each other by construction, and they are what cide passed before this
/// setting existed.
///
/// Notes are produced for a **disabled** injection's bad override too. It is not noise: the
/// screen disables that input when the toggle is off, so the only way to reach one is a
/// hand-edited `workspace.json`, and a rename that will be discarded the moment the toggle
/// comes back on is worth one line rather than a silent surprise later.
///
/// The [`Judged::index`] on a note is the row's position in [`INJECTIONS`], not a position in
/// anything the user typed — these four rows are a fixed table, and the screen finds its row
/// by injection rather than by text.
pub fn injected(cli: &ClaudeCli) -> (Injected, Vec<Judged>) {
    let cfg = &cli.inject;
    let mut notes: Vec<Judged> = Vec::new();

    // Pass one: the shape rule, row by row. `None` means "no usable override; use the default".
    let proposed: Vec<Option<&str>> = INJECTIONS
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let over = (spec.of)(cfg).flag.trim();
            if over.is_empty() {
                return None;
            }
            if !over.starts_with('-') {
                notes.push(Judged {
                    index,
                    text: over.to_string(),
                    verdict: Verdict::Refused(OVERRIDE_NOT_A_FLAG),
                });
                return None;
            }
            Some(over)
        })
        .collect();

    // Pass two: the collision rule, decided against the snapshot above rather than against a
    // list being mutated as it is read. Order-independent on purpose — both halves of a clash
    // lose, and each then falls back to a default that is unique among defaults, so a single
    // pass is enough and there is no second round to reason about.
    let mut resolved: Vec<String> = Vec::with_capacity(INJECTIONS.len());
    for (index, spec) in INJECTIONS.iter().enumerate() {
        let flag = match proposed.get(index).copied().flatten() {
            None => spec.default_flag.to_string(),
            Some(over) => {
                let clashes = INJECTIONS.iter().enumerate().any(|(other, other_spec)| {
                    other != index
                        && (other_spec.default_flag == over
                            || proposed.get(other).copied().flatten() == Some(over))
                });
                if clashes {
                    notes.push(Judged {
                        index,
                        text: over.to_string(),
                        verdict: Verdict::Refused(OVERRIDE_COLLIDES),
                    });
                    spec.default_flag.to_string()
                } else {
                    over.to_string()
                }
            }
        };
        resolved.push(flag);
    }

    let out = INJECTIONS
        .iter()
        .zip(resolved)
        .filter(|(spec, _)| (spec.of)(cfg).enabled)
        .map(|(spec, flag)| (spec.injection, flag))
        .collect();

    (Injected(out), notes)
}

// ==========================================================================================
// The tables.
// ==========================================================================================

/// Arguments cide passes itself, and what a second copy costs.
///
/// Every entry is a flag **cide's own argv already carries or depends on the absence of**, and
/// every one of them fails silently rather than loudly — which is why they are refused rather
/// than left to the CLI to complain about. A flag that merely does something surprising is not
/// in here; the user is allowed to surprise themselves.
///
/// The spellings were read out of `claude --help` on 2.1.233 rather than remembered, including
/// which of them take a value. `--resume`'s value is optional (`-r, --resume [value]`), which is
/// the one that makes [`refused_args`]'s value-swallowing rule a judgement rather than a fact —
/// see there.
///
/// **This is a list of somebody else's flag names and it will go stale**, exactly as
/// `SUPPORTED_CLI` does. `cide_claude::version`'s module header keeps the table of what a patch
/// release can retract; this list is on it, and `cide-claude/tests/real_session_args.rs` is the
/// `#[ignore]`d check that puts these shapes in front of the installed binary. A flag that is
/// renamed upstream stops being refused, which degrades to today's behaviour — the user gets
/// what they asked for and the pane breaks — rather than to a refusal of something harmless.
///
/// # Four of the seven are conditional, and three are not
///
/// [`RefusedArg::because`] carries that distinction, and it is the whole reason the injection
/// switches are safe to have. Four entries here exist *because cide passes the flag*; the
/// moment it stops, the refusal has to lift or the screen would disable a feature and then
/// forbid the replacement. Three — `--bare`, `--print` and `--continue` — are about the flag
/// itself or about a combination, and `--continue` is the one worth reading twice: it is
/// illegal *beside* an injected session id, so it carries `Injection::SessionId` rather than a
/// variant of its own.
pub const REFUSED_ARGS: &[RefusedArg] = &[
    RefusedArg {
        flag: "--session-id",
        aliases: &[],
        takes_value: true,
        reason: "cide passes this itself: the uuid *is* the pane's SessionId, and the hooks \
                 report against it. A second one makes every hook frame name a session this \
                 process has never heard of — no token figures, no busy-versus-idle close \
                 confirm — and writes an id into workspace.json with no transcript behind it. \
                 Settings → Claude sessions → What cide adds to the command line can switch \
                 cide's own off, and this refusal lifts with it.",
        because: Some(Injection::SessionId),
    },
    RefusedArg {
        flag: "--resume",
        aliases: &["-r"],
        takes_value: true,
        reason: "cide passes this itself when a restored pane continues its conversation. A \
                 second one resumes something else under a pane bound to this one, and the \
                 duplicate-session guard keys on cide having chosen it. Settings → Claude \
                 sessions → What cide adds to the command line can switch cide's own off, and \
                 this refusal lifts with it.",
        because: Some(Injection::Resume),
    },
    RefusedArg {
        flag: "--fork-session",
        aliases: &[],
        takes_value: false,
        reason: "Only legal beside a session id or a resume, both of which cide owns. A stray \
                 one changes which conversation the pane *is*, and cide's Split and fork \
                 gesture is what passes it deliberately. Settings → Claude sessions → What \
                 cide adds to the command line can switch cide's own off, and this refusal \
                 lifts with it.",
        because: Some(Injection::ForkSession),
    },
    RefusedArg {
        flag: "--continue",
        aliases: &["-c"],
        takes_value: false,
        reason: "The CLI refuses a session id beside --continue unless --fork-session is also \
                 given, so this does not degrade a feature — every Claude pane fails to start. \
                 It is the session id injection that makes it illegal, so switching that one \
                 off in Settings lifts this refusal too.",
        because: Some(Injection::SessionId),
    },
    RefusedArg {
        flag: "--settings",
        aliases: &[],
        takes_value: true,
        reason: "cide passes this itself, carrying the inline hook payload that makes the \
                 status line, the token figures and the fast buffer reload work. One of the \
                 two loses, and if yours wins every hook dies with nothing on screen saying \
                 so. Put your own settings in ~/.claude/settings.json, which cide never edits \
                 — or switch cide's injection off in Settings, which lifts this refusal and \
                 costs you every one of those features.",
        because: Some(Injection::Settings),
    },
    RefusedArg {
        flag: "--bare",
        aliases: &[],
        takes_value: false,
        reason: "Skips hooks, and makes authentication strictly ANTHROPIC_API_KEY or \
                 apiKeyHelper — OAuth and the keychain are never read. For a Claude Max or Pro \
                 subscriber that is an authentication failure in every pane, with nothing \
                 naming the cause.",
        because: None,
    },
    RefusedArg {
        flag: "--print",
        aliases: &["-p"],
        takes_value: false,
        reason: "Turns an interactive pane into a one-shot that answers and exits. cide has a \
                 headless lane of its own for that — Generate commit message, Explain selection \
                 — and it is not this one.",
        because: None,
    },
];
/// Arguments that are legitimate, cost something cide cannot repair, and are allowed anyway.
///
/// The line between this table and [`REFUSED_ARGS`] is whether the user could plausibly mean it.
/// `--safe-mode` disables hooks the way `--bare` does, and unlike `--bare` it leaves
/// authentication alone and exists precisely to be reached for when a configuration is broken —
/// so refusing it would take away a troubleshooting tool to protect a status line.
pub const WARNED_ARGS: &[RefusedArg] = &[RefusedArg {
    flag: "--safe-mode",
    aliases: &[],
    takes_value: false,
    reason: "Starts with hooks disabled, so this pane reports no session state: the status bar \
             shows no token figures and the close confirmation cannot tell a busy agent from an \
             idle one. The pane itself works.",
    // Nothing cide injects is implicated: this is a flag whose *effect* overlaps the hooks,
    // not a duplicate of one cide passes, so it is warned however the injections are set.
    because: None,
}];

/// One entry of [`REFUSED_ARGS`] or [`WARNED_ARGS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedArg {
    /// The long form, as `claude --help` spells it.
    pub flag: &'static str,
    /// Short forms and spelling variants. Matched exactly, never as a prefix.
    pub aliases: &'static [&'static str],
    /// Whether a following token that is not itself a flag belongs to this one.
    pub takes_value: bool,
    /// One sentence, printed beside the struck-out token. Prose, because the screen prints it.
    ///
    /// **Names the thing, not the token.** These sentences used to embed their own spelling
    /// ("cide passes `--session-id` itself"), which stopped being true the moment the spelling
    /// became configurable — a user who renamed the injection would read a paragraph about a
    /// flag nobody is passing.
    pub reason: &'static str,
    /// The injection this refusal exists *because of*, or `None` for one that stands whatever
    /// cide passes.
    ///
    /// This field is the **one place** the two facts are kept together, and keeping them apart
    /// is the failure it exists to prevent. `--session-id` is refused because cide passes it;
    /// stop passing it and the refusal must relax, or the user has a switch that turns a
    /// feature off and then blocks the replacement — worse than not having the switch. In the
    /// other direction a refusal that relaxed while cide still injected would put the user's
    /// flag and cide's on one command line, which is the silent breakage this whole table was
    /// written for. Both directions are pinned by
    /// `every_injected_flag_is_refused_and_every_injection_refusal_is_injected`.
    ///
    /// `--continue` carries [`Injection::SessionId`] and not a variant of its own: it is
    /// illegal *beside* an injected session id, so it is that injection's refusal rather than
    /// one about `--continue` itself. `--bare` and `--print` carry `None` — they break
    /// authentication and turn a pane into a one-shot regardless of what cide adds.
    pub because: Option<Injection>,
}

impl RefusedArg {
    /// Does `token` name this flag? `--resume`, `-r` and `--resume=x` all do; `--resumed` and
    /// `--no-resume` do not.
    ///
    /// **Whole token, never a substring**, which is the mistake this method exists to not make:
    /// a `contains` here refuses `--resumed-thing` and, worse, refuses `--append-system-prompt`
    /// for containing `-p`. The `=` form is split first because commander accepts it and a user
    /// who writes `--resume=abc` means exactly what `--resume abc` means.
    pub fn matches(&self, token: &str) -> bool {
        let name = token.split_once('=').map_or(token, |(name, _)| name);
        name == self.flag || self.aliases.contains(&name)
    }

    /// Does `token` name `flag`, ignoring this entry's own spelling?
    ///
    /// For a refusal whose flag cide has been told to spell differently. The aliases are
    /// deliberately **not** consulted: `-r` is Claude Code's short form for `--resume`, and a
    /// user who renamed the injection is driving something that is not Claude Code — refusing
    /// `-r` there would block a flag of the other harness's that cide never passes.
    fn matches_as(flag: &str, token: &str) -> bool {
        token.split_once('=').map_or(token, |(name, _)| name) == flag
    }

    /// Did this token carry its value inline, as `--resume=abc`?
    ///
    /// An inline value means the *next* token is not ours to take, which is the difference
    /// between swallowing one token and swallowing two.
    fn inline_value(&self, token: &str) -> bool {
        token.contains('=')
    }
}

/// Environment variables cide sets itself, or must never set.
///
/// Two populations in one table because the user needs one answer — *you cannot set this here* —
/// and the reasons differ. Every name was checked against the strings in the installed CLI
/// (2.1.233) rather than remembered, which is the standard `cide_claude::version`'s table sets:
/// a name cide invents refuses something the CLI has never read, and a name cide misspells
/// refuses nothing at all.
///
/// **`CLAUDE_CODE_SSE_PORT` deliberately does not appear in `child_env.rs` or in
/// `sections.tsx`.** `ui/scripts/check-claude-env.mjs` asserts set-equality between the
/// `CLAUDE_CODE_*` names in those two files — a name on one side and not the other is a switch
/// wired to nothing — and a refusal list is neither a switch nor a spawn. Keeping the table here
/// keeps that gate measuring what it was written to measure.
pub const REFUSED_ENV: &[(&str, &str)] = &[
    (
        "ANTHROPIC_API_KEY",
        "A key outranks subscription OAuth in the CLI's credential order, so setting one here \
         would silently bill a Console organisation for a Claude Max or Pro user. cide never \
         injects one — and never removes one your own login environment carries, which is the \
         only credential some Console customers have.",
    ),
    (
        "ANTHROPIC_AUTH_TOKEN",
        "The same credential by another door, and the same consequence: it outranks the \
         subscription OAuth a pane would otherwise inherit.",
    ),
    (
        "CLAUDE_CODE_SSE_PORT",
        "cide sets this per project. It is what makes a pane's claude find *this* editor's \
         lockfile — a wrong value has it bind to another editor entirely, and the inline diffs, \
         @-mentions and editor selection silently stop working.",
    ),
    (
        "CIDE_HOOK_SOCK",
        "cide's own hook socket. A pane that cannot reach it reports no session state at all.",
    ),
    (
        "TERM",
        "cide sets xterm-256color. Plain xterm costs the TUI its 256 colours, its box-drawing \
         and the alternate screen.",
    ),
    (
        "COLUMNS",
        "The pane's size comes from the PTY. A fixed value lies to the child until the first \
         resize.",
    ),
    (
        "LINES",
        "The pane's size comes from the PTY. A fixed value lies to the child until the first \
         resize.",
    ),
    (
        "TMUX",
        "Its presence triggers an unconditional 256-colour clamp that visibly desaturates the \
         accent colour. cide removes it.",
    ),
    (
        "CLAUDE_CODE_SCROLL_SPEED",
        "There is a control for this above, and two controls writing one variable is how the \
         one you can see silently loses.",
    ),
    (
        "CLAUDE_CODE_DISABLE_MOUSE",
        "There is a switch for this above, and it removes the variable when it is off — so a \
         value set here would be applied first and then deleted, from a control you cannot see.",
    ),
    (
        "CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT",
        "There is a switch for this above, and it removes the variable when it is off — so a \
         value set here would be applied first and then deleted, from a control you cannot see.",
    ),
    (
        "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN",
        "There is a switch for this above, and it removes the variable when it is off — so a \
         value set here would be applied first and then deleted, from a control you cannot see.",
    ),
    (
        "HTTP_PROXY",
        "Proxying has a screen of its own, with three modes and three scopes so that “no proxy” \
         and “do not interfere” stay distinguishable. Set it there; it reaches claude panes and \
         the one-shot lane together.",
    ),
    (
        "HTTPS_PROXY",
        "Proxying has a screen of its own, and it is applied after this list — so a value here \
         would be overwritten without ever reaching a child. Set it there.",
    ),
    (
        "ALL_PROXY",
        "Proxying has a screen of its own, and it is applied after this list — so a value here \
         would be overwritten without ever reaching a child. Set it there.",
    ),
    (
        "NO_PROXY",
        "Proxying has a screen of its own, and it is applied after this list — so a value here \
         would be overwritten without ever reaching a child. Set it there. Loopback is always \
         exempt and cannot be removed.",
    ),
    (
        "CLAUDE_CONFIG_DIR",
        "cide reads this from its *own* environment to find the transcript behind a pane. Set \
         for the child alone, the two disagree: every Resume button vanishes, or offers a resume \
         that fails. Export it before launching cide instead, and both sides agree.",
    ),
];

/// Variables that work, mean something serious, and are allowed with a sentence.
///
/// The line is whether cide's own behaviour becomes wrong. Billing against a cloud account or a
/// gateway is the user's decision to make and theirs to see on an invoice; a `CLAUDE_CONFIG_DIR`
/// that only the child agrees with makes *cide* wrong about what is resumable, which is why that
/// one is in the table above instead.
///
/// Names checked against the installed CLI's own strings, as above.
pub const WARNED_ENV: &[(&str, &str)] = &[
    (
        "ANTHROPIC_BASE_URL",
        "Sends every request to another endpoint. That is a legitimate gateway configuration and \
         also the shape of a credential redirect — cide cannot tell them apart, so it is your \
         call.",
    ),
    (
        "CLAUDE_CODE_USE_BEDROCK",
        "Bills an AWS account rather than your subscription, using Bedrock's own credentials.",
    ),
    (
        "CLAUDE_CODE_USE_VERTEX",
        "Bills a Google Cloud account rather than your subscription, using Vertex's own \
         credentials.",
    ),
];

// ==========================================================================================
// The verdicts.
// ==========================================================================================

/// What cide will do with one thing the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Passed to the child unchanged.
    Accepted,
    /// Passed to the child, with something to say about it.
    Warned(&'static str),
    /// Not passed to the child, with the reason.
    Refused(&'static str),
}

impl Verdict {
    pub fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }

    /// The sentence, when there is one.
    pub fn note(&self) -> Option<&'static str> {
        match self {
            Self::Accepted => None,
            Self::Warned(note) | Self::Refused(note) => Some(note),
        }
    }
}

/// One entry of the user's list, and what became of it.
///
/// Carries the *index* into the stored list so the Settings screen can strike out the row the
/// user is looking at rather than the first row whose text happens to match. Two identical
/// tokens are ordinary — `--add-dir a --add-dir b` — and matching on text would mark the wrong
/// one when only the second is swallowed as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub index: usize,
    /// Exactly what the user wrote. Never normalised — a user has to be able to recognise their
    /// own value in the readout in order to correct it.
    pub text: String,
    pub verdict: Verdict,
}

/// Everything a spawn needs, and everything the screen prints, from one pass.
///
/// One type rather than a function per consumer because the interesting property is that the
/// screen and the spawn agree, and two functions is two chances for them not to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Argument tokens to place **before** every argument cide adds. See [`user_args`].
    pub args: Vec<String>,
    /// Environment changes to fold after [`crate::child_env::claude_env`] and before the proxy.
    ///
    /// Always `Some(value)`; see [`user_env`].
    pub env: Vec<EnvChange>,
    /// Every argument row, in the order the user wrote them.
    pub arg_notes: Vec<Judged>,
    /// Every environment row, in the order the user wrote them.
    pub env_notes: Vec<Judged>,
    /// The arguments cide will add itself, resolved: which of them, spelled how.
    ///
    /// In `Plan` rather than resolved again at the spawn, so the set folded into the argv is
    /// **the same value** the verdicts above were computed against. Two resolutions is two
    /// chances to refuse a user's `--session-id` while passing none of cide's, which is the
    /// one outcome the switches must never produce — the same argument this type's own doc
    /// makes about the screen and the spawn.
    ///
    /// `Plan::default()` injects nothing, which is what a shell pane gets.
    pub inject: Injected,
    /// Injection overrides that were discarded, and why. See [`injected`].
    pub inject_notes: Vec<Judged>,
}

impl Plan {
    /// Rows the child never sees. What the log line names, and what the screen strikes out.
    pub fn refusals(&self) -> impl Iterator<Item = &Judged> {
        self.arg_notes
            .iter()
            .chain(&self.env_notes)
            // The discarded renames belong here too: a spawn that quietly used the default
            // spelling because the override was a positional would otherwise be the one
            // refusal with no line anywhere, and `workspace.json` is hand-editable.
            .chain(&self.inject_notes)
            .filter(|judged| judged.verdict.is_refused())
    }
}

/// The refusal that covers this token, given what cide is actually going to inject.
///
/// # The three ways an entry is matched, and why they differ
///
/// * `because: None` — matched on its own name and aliases, always. `--bare` and `--print`
///   break a pane whatever cide adds.
/// * `because: Some(i)` where the entry *is* the injected flag — matched on the **effective**
///   spelling, and not at all when `i` is switched off. Rename the session id injection to
///   `--sid` and `--sid` becomes the refused token while `--session-id` becomes the user's to
///   pass; that is the whole point of deriving one from the other.
/// * `because: Some(i)` where the entry is a *different* flag that is illegal beside the
///   injected one — `--continue` beside a session id. Matched on its own name, because it is
///   the CLI's flag rather than cide's, and merely gated on the injection being on.
fn refusal_for(token: &str, injected: &Injected) -> Option<&'static RefusedArg> {
    REFUSED_ARGS.iter().find(|entry| match entry.because {
        None => entry.matches(token),
        Some(which) => match injected.flag(which) {
            None => false,
            Some(effective) => {
                let default = spec_of(which).default_flag;
                if entry.flag == default && effective != default {
                    RefusedArg::matches_as(effective, token)
                } else {
                    entry.matches(token)
                }
            }
        },
    })
}

/// The verdict on one argument token, ignoring its neighbours.
///
/// Neighbours matter — a refused `--resume` takes the `abc` after it — which is why the whole
/// list goes through [`user_args`] and this answers only about the token itself.
///
/// Takes the resolved [`Injected`] rather than reading the configuration itself, so a caller
/// cannot ask about a set of flags different from the one about to be written. Pass
/// [`Injected::defaults`] for "what cide has always injected".
pub fn arg_verdict(token: &str, injected: &Injected) -> Verdict {
    if let Some(entry) = refusal_for(token, injected) {
        return Verdict::Refused(entry.reason);
    }
    if let Some(entry) = WARNED_ARGS.iter().find(|entry| entry.matches(token)) {
        return Verdict::Warned(entry.reason);
    }
    Verdict::Accepted
}

/// The verdict on one environment variable.
///
/// `appdir` is this process's `APPDIR` when cide was launched from an AppImage and `None`
/// otherwise, which is every development run, every `.deb` and every Flatpak. Passed in rather
/// than read so this stays pure: `std::env::set_var` is `unsafe` in edition 2024 precisely
/// because it races every other thread, and a test that needed the real process environment
/// could not be written safely at all — which is the same reason [`crate::child_env::
/// bundle_scrub_from`] takes its environment as an argument.
pub fn env_verdict(name: &str, value: &str, appdir: Option<&str>) -> Verdict {
    // Trimmed for the comparison only. A user who types a trailing space into the name field
    // means the name, and a refusal that can be sidestepped by a space is not a refusal.
    let trimmed = name.trim();
    if let Some((_, reason)) = REFUSED_ENV
        .iter()
        .find(|(refused, _)| refused.eq_ignore_ascii_case(trimmed))
    {
        return Verdict::Refused(reason);
    }
    // The bundle rule, run over this one pair. Not a name list — see the module header, and
    // `child_env`'s, which makes the argument at length: a variable a future AppImage plugin
    // invents has the same shape and no name anybody here could have written down.
    //
    // Anything `bundle_scrub_from` wants to *change* is a value that would reintroduce ADR
    // 0007's bug, so the test is that it wants to change nothing. That includes the bundle
    // markers (`APPDIR`, `APPIMAGE`, `ARGV0`, `OWD`), which it removes outright.
    if let Some(appdir) = appdir
        && !crate::child_env::bundle_scrub_from([(trimmed.to_string(), value.to_string())], appdir)
            .is_empty()
    {
        return Verdict::Refused(
            "This value points inside cide's own AppImage, which will not exist once cide \
             exits. Passing one on is what kills every stdio MCP server a pane's claude \
             starts — reported three processes away as CONNECTION_CLOSED against a \
             configuration that is perfectly correct. See docs/adr/0007.",
        );
    }
    if let Some((_, reason)) = WARNED_ENV
        .iter()
        .find(|(warned, _)| warned.eq_ignore_ascii_case(trimmed))
    {
        return Verdict::Warned(reason);
    }
    Verdict::Accepted
}

/// The argument tokens that survive, and a note per row.
///
/// # A refused flag takes its value with it
///
/// Dropping `--resume` and keeping `abc` would be worse than doing nothing: `claude`'s first
/// positional argument is a *prompt*, so the orphan would not be ignored — it would start every
/// pane by asking the model something. So a refused flag that takes a value also swallows the
/// next token when that token is not itself a flag.
///
/// That is a judgement and not a fact, because `-r, --resume [value]` takes an *optional* one:
/// a user who writes `--resume` and then, as a separate row, a positional they meant to keep,
/// loses it. The readout is what makes that survivable — it prints the resulting argv, and the
/// swallowed row is struck out with the same reason as the flag that took it. The alternative,
/// leaving the orphan, is silent and starts a turn.
///
/// A token written as `--resume=abc` carries its own value and swallows nothing.
pub fn user_args(cli: &ClaudeCli) -> (Vec<String>, Vec<Judged>) {
    user_args_in(cli, &injected(cli).0)
}

/// [`user_args`], against an [`Injected`] the caller has already resolved.
///
/// Exists so [`plan`] resolves the injections **once** and judges the user's tokens against
/// the same value it puts in `Plan::inject`. Not public: a caller who could pass an unrelated
/// `Injected` could produce a readout about a command line nothing will spawn.
fn user_args_in(cli: &ClaudeCli, injected: &Injected) -> (Vec<String>, Vec<Judged>) {
    let mut kept = Vec::new();
    let mut notes: Vec<Judged> = Vec::with_capacity(cli.args.len());
    // Set when the previous token was refused and is still owed a value.
    let mut swallow: Option<&'static str> = None;

    for (index, token) in cli.args.iter().enumerate() {
        if let Some(reason) = swallow.take()
            && !token.starts_with('-')
        {
            // A refused flag's value, taken with it. See this function's own note for why the
            // orphan cannot simply be left: `claude`'s first positional argument is a prompt.
            notes.push(Judged {
                index,
                text: token.clone(),
                verdict: Verdict::Refused(reason),
            });
            continue;
        }

        let verdict = arg_verdict(token, injected);
        if let Verdict::Refused(reason) = verdict
            && let Some(entry) = refusal_for(token, injected)
            && entry.takes_value
            && !entry.inline_value(token)
        {
            swallow = Some(reason);
        }
        if !verdict.is_refused() {
            kept.push(token.clone());
        }
        notes.push(Judged {
            index,
            text: token.clone(),
            verdict,
        });
    }

    (kept, notes)
}

/// The environment changes that survive, and a note per row.
///
/// # Every change is a set, never a removal
///
/// The list holds `NAME=value` pairs and an empty value means the variable is set to the empty
/// string, which is a thing a program can distinguish from unset and a thing users sometimes
/// want. There is deliberately no way to spell *remove* here, and the reason is the second
/// non-negotiable rule: a removal control would let a user delete `ANTHROPIC_API_KEY` from the
/// environment they launched cide with, which is the only credential a Console customer may
/// have. Refusing to inject one and refusing to remove one are the same promise from two sides.
///
/// A row whose name is blank contributes nothing and is not an error: it is what a freshly
/// added row looks like before the user has typed into it.
///
/// # Duplicates are kept, and the last one wins
///
/// Which is what `SpawnSpec::env` actually implements — `cide-pty` applies every `env_remove`
/// first and then each `env` in insertion order. Collapsing them here would make the readout
/// disagree with the child, and the readout is the whole reason this is visible at all.
pub fn user_env(cli: &ClaudeCli, appdir: Option<&str>) -> (Vec<EnvChange>, Vec<Judged>) {
    let mut kept = Vec::new();
    let mut notes = Vec::with_capacity(cli.env.len());

    for (index, var) in cli.env.iter().enumerate() {
        let name = var.name.trim();
        if name.is_empty() {
            continue;
        }
        let verdict = env_verdict(name, &var.value, appdir);
        if !verdict.is_refused() {
            kept.push((name.to_string(), Some(var.value.clone())));
        }
        notes.push(Judged {
            index,
            text: name.to_string(),
            verdict,
        });
    }

    (kept, notes)
}

/// Both halves in one pass, for a spawn site that wants the survivors and a log line.
pub fn plan(cli: &ClaudeCli, appdir: Option<&str>) -> Plan {
    // Resolved first and threaded through both halves: the verdict on a user's `--session-id`
    // and the decision to write cide's own are one question asked once.
    let (inject, inject_notes) = injected(cli);
    let (args, arg_notes) = user_args_in(cli, &inject);
    let (env, env_notes) = user_env(cli, appdir);
    Plan {
        args,
        env,
        arg_notes,
        env_notes,
        inject,
        inject_notes,
    }
}

/// [`plan`], reading this process's `APPDIR` for the bundle rule. What a spawn site calls.
pub fn plan_here(cli: &ClaudeCli) -> Plan {
    plan(cli, std::env::var("APPDIR").ok().as_deref())
}

// ==========================================================================================
// The binary.
// ==========================================================================================

/// Why a configured `claude` cannot be run.
///
/// Only the three things the OS cannot `exec`. Everything else — a wrapper script, a shim, a
/// binary whose `--version` prints nothing cide can parse — is a *warning*, because `mise`,
/// `asdf`, `direnv` and a plain shell wrapper are all legitimate ways to name a `claude` and
/// none of them answers `--version` in a shape a parser should be asked to trust. The project
/// has already paid for the opposite arrangement once: `~/.cargo/bin/rust-analyzer` is a
/// symlink to `rustup`, so it passes any on-PATH-and-executable probe and then fails at exec,
/// and the fix there was to keep the probe *and* translate the exec failure into a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryProblem {
    /// The field is empty. Distinguished from "not found" because the remedy is different and
    /// because an empty program string reaches `execvp` as an immediate `ENOENT` with no name
    /// in the message.
    Blank,
    /// A bare name that is on no directory of [`crate::toolchain::search_paths`].
    NotOnPath { name: String },
    /// A path that does not exist, is a directory, or has no execute bit.
    NotExecutable { path: PathBuf },
}

impl BinaryProblem {
    /// One sentence, for the Settings note and for the pane's own transcript.
    ///
    /// The same words in both places on purpose: a user who reads *claude: not found on PATH* in
    /// a dead pane and then opens Settings should see the sentence they already read, not a
    /// second wording of it that leaves them wondering whether it is a second problem.
    pub fn message(&self) -> String {
        match self {
            Self::Blank => "No Claude binary is configured. Settings → Claude sessions → \
                            Binary; “claude” is the default and lets PATH resolve it."
                .to_string(),
            Self::NotOnPath { name } => format!(
                "“{name}” is not on this app's PATH, nor in ~/.cargo/bin or ~/go/bin. A cide \
                 started from a desktop launcher has a different PATH from one started in a \
                 terminal, so give an absolute path in Settings → Claude sessions → Binary if \
                 it works in your shell."
            ),
            Self::NotExecutable { path } => format!(
                "“{}” is not an executable file. Settings → Claude sessions → Binary.",
                path.display()
            ),
        }
    }
}

/// Can this configured binary be run, and where does it resolve to?
///
/// The resolved path is for *display and probing only*. What a spawn passes is
/// [`cide_ipc::ClaudeCli::binary`] exactly as stored — a bare name stays a bare name so the OS
/// resolves it at each spawn, because the CLI updates itself underneath a running app and
/// pinning the answer at launch would keep a pane on a version that has been replaced. Resolving
/// it here and spawning that path would look identical for weeks and then be subtly wrong.
///
/// Touches the disk (a `stat` per `PATH` entry at worst) and must not run on the GTK loop.
pub fn resolve(binary: &str) -> Result<PathBuf, BinaryProblem> {
    let binary = binary.trim();
    if binary.is_empty() {
        return Err(BinaryProblem::Blank);
    }
    // A name with no separator is a `PATH` lookup; anything else is a path, relative or
    // absolute. This is `execvp`'s own rule, so cide's verdict and the kernel's agree about
    // which of the two errors a user is looking at.
    if !binary.contains('/') {
        return crate::toolchain::which(binary).ok_or_else(|| BinaryProblem::NotOnPath {
            name: binary.to_string(),
        });
    }
    let path = PathBuf::from(binary);
    if crate::toolchain::is_executable(&path) {
        Ok(path)
    } else {
        Err(BinaryProblem::NotExecutable { path })
    }
}

/// Whether the program about to be spawned is the Claude CLI, for a caller that has already
/// substituted the configured binary.
///
/// Deliberately *not* a file-name match on the resolved program, which is what
/// `cmd::session.rs::program_is_claude` does and what its own note warns about: `claude` on this
/// machine resolves to `…/claude/versions/2.1.233`, whose file name is a version number. The
/// decision has to be made from what the *frontend asked for* and then carried, which is why
/// this takes a bool rather than a path — it exists to be named at the call site, so a reader of
/// either end finds the rule.
pub fn program_for(is_claude: bool, configured: &str, requested: &Path) -> PathBuf {
    if is_claude {
        PathBuf::from(configured.trim())
    } else {
        requested.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{ClaudeEnvVar, ClaudeSettings};

    fn cli(args: &[&str], env: &[(&str, &str)]) -> ClaudeCli {
        ClaudeCli {
            binary: "claude".into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: env
                .iter()
                .map(|(name, value)| ClaudeEnvVar {
                    name: (*name).into(),
                    value: (*value).into(),
                })
                .collect(),
            // `..Default::default()` rather than a literal, so a fifth injection added to the
            // DTO does not need an edit here — and so these tests keep asking about the
            // *shipped* default, which is the property most of them are about.
            ..Default::default()
        }
    }

    /// A `ClaudeCli` with one injection switched off.
    fn without(which: Injection) -> ClaudeCli {
        let mut cli = ClaudeCli::default();
        field_mut(&mut cli, which).enabled = false;
        cli
    }

    /// A `ClaudeCli` with one injection renamed.
    fn renamed_cli(which: Injection, flag: &str) -> ClaudeCli {
        let mut cli = ClaudeCli::default();
        field_mut(&mut cli, which).flag = flag.to_string();
        cli
    }

    /// The mutable half of `InjectionSpec::of`, which the shipping code has no use for: it
    /// only ever reads the configuration. Written out here rather than added to the table so
    /// the table stays the read-only description it is.
    fn field_mut(cli: &mut ClaudeCli, which: Injection) -> &mut ClaudeInjection {
        match which {
            Injection::SessionId => &mut cli.inject.session_id,
            Injection::Resume => &mut cli.inject.resume,
            Injection::ForkSession => &mut cli.inject.fork_session,
            Injection::Settings => &mut cli.inject.settings,
        }
    }

    fn flags(cli: &ClaudeCli) -> Vec<String> {
        injected(cli)
            .0
            .iter()
            .map(|(_, flag)| flag.to_string())
            .collect()
    }

    /// Nothing of the *user's* is added by default — and everything of cide's still is.
    ///
    /// The second half used to be `plan == Plan::default()`, which stopped being the right
    /// assertion the moment `Plan` carried the injections: `Plan::default()` injects nothing,
    /// which is what a shell pane gets. Spelled out explicitly instead, so a change to what
    /// cide passes out of the box is a failing test rather than a silent one.
    #[test]
    fn the_default_adds_nothing_of_the_users_and_everything_of_cides() {
        let plan = plan(&ClaudeCli::default(), None);
        assert!(plan.args.is_empty());
        assert!(plan.env.is_empty());
        assert!(plan.arg_notes.is_empty());
        assert!(plan.env_notes.is_empty());
        assert!(plan.inject_notes.is_empty());
        assert_eq!(plan.inject, Injected::defaults());
        assert_eq!(
            plan.inject
                .iter()
                .map(|(_, flag)| flag)
                .collect::<Vec<&str>>(),
            ["--session-id", "--resume", "--fork-session", "--settings"],
            "the default configuration spells cide's own arguments exactly as it always has"
        );
        assert_eq!(ClaudeSettings::default().cli.binary, "claude");
    }

    // ======================================================================================
    // The injections, and the relationship the design hangs on.
    // ======================================================================================

    /// The property the whole feature rests on: the flags cide injects and the flags it
    /// refuses *for an injection reason* are the same set, in both directions.
    ///
    /// Two lists would be two chances to disable an injection and leave its refusal standing —
    /// a switch that turns a feature off and then forbids the replacement — or to relax a
    /// refusal while cide still injects, which puts the user's flag and cide's on one command
    /// line and is the silent breakage `REFUSED_ARGS` was written for.
    ///
    /// Delete a `because` from a row that has one and
    /// `disabling_an_injection_relaxes_exactly_its_own_refusals` fails; delete an injection's
    /// refusal outright, or add an injection with none, and this one does.
    #[test]
    fn every_injected_flag_is_refused_and_every_injection_refusal_is_injected() {
        for spec in INJECTIONS {
            assert!(
                REFUSED_ARGS
                    .iter()
                    .any(|entry| entry.because == Some(spec.injection)
                        && entry.flag == spec.default_flag),
                "cide injects {} and nothing refuses it: a user could pass a second copy",
                spec.default_flag
            );
        }
        for entry in REFUSED_ARGS {
            let Some(which) = entry.because else { continue };
            assert!(
                INJECTIONS.iter().any(|spec| spec.injection == which),
                "{} is refused because of an injection that does not exist",
                entry.flag
            );
        }
        // And the unconditional ones stay unconditional. A `because` added to `--bare` would
        // make an authentication failure switchable from a screen about spelling.
        for flag in ["--bare", "--print"] {
            assert!(
                REFUSED_ARGS
                    .iter()
                    .find(|entry| entry.flag == flag)
                    .is_some_and(|entry| entry.because.is_none()),
                "{flag} breaks a pane whatever cide passes, so it is not an injection's refusal"
            );
        }
    }

    #[test]
    fn disabling_an_injection_relaxes_exactly_its_own_refusals() {
        let off = injected(&without(Injection::SessionId)).0;
        for token in ["--session-id", "--session-id=abc", "--continue", "-c"] {
            assert_eq!(
                arg_verdict(token, &off),
                Verdict::Accepted,
                "{token} is illegal only beside a session id cide passes, and it no longer does"
            );
        }
        for token in [
            "--resume",
            "-r",
            "--fork-session",
            "--settings",
            "--bare",
            "--print",
        ] {
            assert!(
                arg_verdict(token, &off).is_refused(),
                "{token} has nothing to do with the session id injection"
            );
        }

        // The settings injection, whose refusal is the expensive one to get wrong in either
        // direction: relaxed while cide still passes it, both payloads land and one loses.
        let off = injected(&without(Injection::Settings)).0;
        assert_eq!(arg_verdict("--settings", &off), Verdict::Accepted);
        assert!(arg_verdict("--session-id", &off).is_refused());
    }

    #[test]
    fn renaming_an_injection_moves_the_refusal_to_the_new_spelling() {
        let renamed = injected(&renamed_cli(Injection::SessionId, "--sid")).0;
        assert_eq!(renamed.flag(Injection::SessionId), Some("--sid"));
        assert!(
            arg_verdict("--sid", &renamed).is_refused(),
            "the flag cide now writes is the one a second copy of would break"
        );
        assert!(
            arg_verdict("--sid=abc", &renamed).is_refused(),
            "and the `=` form of it"
        );
        assert_eq!(
            arg_verdict("--session-id", &renamed),
            Verdict::Accepted,
            "and the spelling cide has stopped writing is the user's to pass"
        );
        // The alias goes with the flag it was an alias *for*. A renamed injection is driving
        // something that is not Claude Code, and `-r` there is that harness's flag, not ours.
        let renamed = injected(&renamed_cli(Injection::Resume, "--continue-from")).0;
        assert!(arg_verdict("--continue-from", &renamed).is_refused());
        assert_eq!(arg_verdict("-r", &renamed), Verdict::Accepted);
        assert_eq!(arg_verdict("--resume", &renamed), Verdict::Accepted);
    }

    /// `claude`'s first positional argument is a prompt, which is the same trap `user_args`'
    /// value-swallowing rule exists for. An override of `sid` would not rename a flag; it
    /// would start every pane by asking the model something.
    #[test]
    fn a_positional_override_is_discarded_because_claudes_first_positional_is_a_prompt() {
        let cli = renamed_cli(Injection::SessionId, "sid");
        let (inject, notes) = injected(&cli);
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].verdict.is_refused());
        assert_eq!(notes[0].text, "sid");
        assert!(
            notes[0]
                .verdict
                .note()
                .unwrap_or_default()
                .contains("prompt"),
            "the discard says why, or the field silently ignores what was typed into it"
        );
    }

    #[test]
    fn two_injections_cannot_be_given_the_same_spelling() {
        // Against another injection's *default*, which is reserved whether or not that
        // injection is switched on.
        let cli = renamed_cli(Injection::SessionId, "--resume");
        let (inject, notes) = injected(&cli);
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert_eq!(inject.flag(Injection::Resume), Some("--resume"));
        assert_eq!(notes.len(), 1);

        // And against another injection's override. Both lose: picking a winner would make the
        // outcome depend on a table order nobody can see.
        let mut cli = ClaudeCli::default();
        field_mut(&mut cli, Injection::SessionId).flag = "--id".into();
        field_mut(&mut cli, Injection::Settings).flag = "--id".into();
        let (inject, notes) = injected(&cli);
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert_eq!(inject.flag(Injection::Settings), Some("--settings"));
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|note| note.verdict.is_refused()));

        // Even a *swap*, whose two spellings would in fact stay unique. Reserving the default
        // spellings unconditionally is what makes the fallback well-founded: a discarded
        // override falls back to its own default, and that default can then never collide with
        // an override that was accepted. The alternative — allow the swap, and chase whether
        // each fallback re-collides — is a fixpoint loop over a settings field, to buy a
        // configuration nobody has asked for. Both rows are discarded and both say so.
        let mut cli = ClaudeCli::default();
        field_mut(&mut cli, Injection::SessionId).flag = "--resume".into();
        field_mut(&mut cli, Injection::Resume).flag = "--session-id".into();
        let (inject, notes) = injected(&cli);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert_eq!(inject.flag(Injection::Resume), Some("--resume"));
    }

    /// Defaults preserve today's behaviour exactly. Flip `ClaudeInjection::default()` to
    /// `enabled: false` and this fails, which is the point: the hazard is silent, because a
    /// pane with no injections starts perfectly well and simply reports nothing.
    #[test]
    fn the_default_injects_exactly_what_this_build_injected_before_the_switch_existed() {
        let (inject, notes) = injected(&ClaudeCli::default());
        assert!(notes.is_empty());
        assert_eq!(inject, Injected::defaults());
        assert_eq!(
            flags(&ClaudeCli::default()),
            ["--session-id", "--resume", "--fork-session", "--settings"]
        );
        for spec in INJECTIONS {
            assert!(
                inject.has(spec.injection),
                "{} is off by default: every user's hooks or resume would die on upgrade",
                spec.default_flag
            );
        }
    }

    /// The wire keys are the field names of `ClaudeInjections`, which is what the Settings row
    /// looks a discarded rename's sentence up by. A typo is a sentence that never appears.
    #[test]
    fn every_injection_names_the_field_that_configures_it() {
        let value = serde_json::to_value(ClaudeInjections::default()).expect("serializes");
        let fields = value.as_object().expect("a struct");
        assert_eq!(fields.len(), INJECTIONS.len());
        for spec in INJECTIONS {
            assert!(
                fields.contains_key(spec.key),
                "`{}` is not a field of ClaudeInjections",
                spec.key
            );
        }
    }

    /// A disabled injection is absent from the argv and nothing else changes.
    #[test]
    fn a_disabled_injection_is_simply_not_written() {
        let (inject, notes) = injected(&without(Injection::Settings));
        assert!(notes.is_empty());
        assert_eq!(inject.flag(Injection::Settings), None);
        assert!(!inject.has(Injection::Settings));
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert_eq!(flags(&without(Injection::Settings)).len(), 3);
    }

    /// A rename on a switched-off injection still gets its note, because the only way to
    /// reach one is a hand-edited `workspace.json` and it will bite the moment the toggle
    /// comes back on.
    #[test]
    fn a_bad_override_on_a_disabled_injection_is_still_reported() {
        let mut cli = ClaudeCli::default();
        field_mut(&mut cli, Injection::Resume).enabled = false;
        field_mut(&mut cli, Injection::Resume).flag = "resume-please".into();
        let (inject, notes) = injected(&cli);
        assert!(!inject.has(Injection::Resume));
        assert_eq!(notes.len(), 1);
    }

    /// The discarded overrides reach the log line, so a hand-edited `workspace.json` is not
    /// the one refusal nothing anywhere mentions.
    #[test]
    fn a_discarded_override_is_one_of_the_plans_refusals() {
        let plan = plan(&renamed_cli(Injection::Settings, "settings"), None);
        assert_eq!(plan.refusals().count(), 1);
        assert_eq!(plan.inject.flag(Injection::Settings), Some("--settings"));
    }

    /// Whitespace is trimmed, and a whitespace-only override is simply "no override" rather
    /// than a refusal — it is what a text input holds after the user clears it.
    #[test]
    fn an_override_is_trimmed_and_a_blank_one_is_not_an_error() {
        let (inject, notes) = injected(&renamed_cli(Injection::SessionId, "  --sid  "));
        assert_eq!(inject.flag(Injection::SessionId), Some("--sid"));
        assert!(notes.is_empty());
        let (inject, notes) = injected(&renamed_cli(Injection::SessionId, "   "));
        assert_eq!(inject.flag(Injection::SessionId), Some("--session-id"));
        assert!(notes.is_empty());
    }

    /// The whole point of the argument table, one row at a time.
    #[test]
    fn every_argument_cide_passes_itself_is_refused() {
        for token in [
            "--session-id",
            "--resume",
            "-r",
            "--fork-session",
            "--continue",
            "-c",
            "--settings",
            "--bare",
            "--print",
            "-p",
        ] {
            assert!(
                arg_verdict(token, &Injected::defaults()).is_refused(),
                "{token} duplicates an argument cide passes itself"
            );
        }
    }

    /// A substring match here would refuse half the CLI's flags, `--append-system-prompt` among
    /// them (it contains `-p`). Whole tokens only.
    #[test]
    fn a_flag_that_merely_contains_a_refused_one_is_accepted() {
        for token in [
            "--append-system-prompt",
            "--resumed-thing",
            "--no-resume",
            "--settings-file",
            "--printer",
            "--model",
            "opus",
            "--permission-mode",
            "plan",
        ] {
            assert_eq!(
                arg_verdict(token, &Injected::defaults()),
                Verdict::Accepted,
                "{token} is not one of cide's own arguments"
            );
        }
    }

    #[test]
    fn the_equals_form_is_the_same_flag() {
        assert!(arg_verdict("--resume=abc", &Injected::defaults()).is_refused());
        assert!(arg_verdict("--session-id=1234", &Injected::defaults()).is_refused());
        // …and it carries its own value, so nothing after it is swallowed.
        let (kept, notes) = user_args(&cli(&["--resume=abc", "--model", "opus"], &[]));
        assert_eq!(kept, ["--model", "opus"]);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[1].verdict, Verdict::Accepted);
    }

    /// The orphan-positional trap. Dropping `--resume` and keeping `abc` starts every pane by
    /// asking the model something, because `claude`'s first positional argument is a prompt.
    #[test]
    fn a_refused_flag_takes_its_value_with_it() {
        let (kept, notes) = user_args(&cli(&["--model", "opus", "--resume", "abc"], &[]));
        assert_eq!(
            kept,
            ["--model", "opus"],
            "`abc` must not survive as a positional — it would be a prompt"
        );
        assert!(notes[2].verdict.is_refused());
        assert!(
            notes[3].verdict.is_refused(),
            "and the swallowed value is struck out too, with the same reason"
        );
        assert_eq!(notes[3].verdict.note(), notes[2].verdict.note());
    }

    #[test]
    fn a_valueless_refused_flag_swallows_nothing() {
        let (kept, _) = user_args(&cli(&["--fork-session", "hello", "--model", "opus"], &[]));
        assert_eq!(kept, ["hello", "--model", "opus"]);
    }

    #[test]
    fn a_refused_flag_followed_by_another_flag_swallows_nothing() {
        let (kept, _) = user_args(&cli(&["--resume", "--model", "opus"], &[]));
        assert_eq!(kept, ["--model", "opus"]);
    }

    /// Warned, not refused: the token reaches the child and the screen says what it costs.
    #[test]
    fn safe_mode_is_warned_and_still_passed() {
        let (kept, notes) = user_args(&cli(&["--safe-mode"], &[]));
        assert_eq!(kept, ["--safe-mode"]);
        assert!(matches!(notes[0].verdict, Verdict::Warned(_)));
        assert!(!notes[0].verdict.is_refused());
    }

    #[test]
    fn the_credential_names_are_refused_in_any_case() {
        for name in [
            "ANTHROPIC_API_KEY",
            "anthropic_api_key",
            "Anthropic_Api_Key",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            assert!(
                env_verdict(name, "sk-ant-whatever", None).is_refused(),
                "{name} outranks subscription OAuth"
            );
        }
    }

    /// A refusal a trailing space defeats is not a refusal.
    #[test]
    fn whitespace_around_a_refused_name_does_not_smuggle_it_through() {
        assert!(env_verdict(" ANTHROPIC_API_KEY ", "sk-ant", None).is_refused());
        let (kept, notes) = user_env(&cli(&[], &[(" ANTHROPIC_API_KEY ", "sk-ant")]), None);
        assert!(kept.is_empty());
        assert!(notes[0].verdict.is_refused());
    }

    #[test]
    fn the_variables_cide_owns_are_refused() {
        for name in [
            "CLAUDE_CODE_SSE_PORT",
            "CIDE_HOOK_SOCK",
            "TERM",
            "COLUMNS",
            "LINES",
            "TMUX",
            "CLAUDE_CODE_SCROLL_SPEED",
            "CLAUDE_CODE_DISABLE_MOUSE",
            "HTTPS_PROXY",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(env_verdict(name, "1", None).is_refused(), "{name}");
        }
    }

    #[test]
    fn a_gateway_or_a_cloud_provider_is_warned_and_still_set() {
        for name in [
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ] {
            assert!(
                matches!(env_verdict(name, "1", None), Verdict::Warned(_)),
                "{name} is the user's decision, and theirs to see on an invoice"
            );
        }
        let (kept, _) = user_env(
            &cli(&[], &[("ANTHROPIC_BASE_URL", "https://gw.corp")]),
            None,
        );
        assert_eq!(
            kept,
            [(
                "ANTHROPIC_BASE_URL".to_string(),
                Some("https://gw.corp".to_string())
            )]
        );
    }

    #[test]
    fn an_ordinary_variable_is_passed_through() {
        let (kept, notes) = user_env(&cli(&[], &[("MY_MCP_TOKEN", "hunter2")]), None);
        assert_eq!(
            kept,
            [("MY_MCP_TOKEN".to_string(), Some("hunter2".to_string()))]
        );
        assert_eq!(notes[0].verdict, Verdict::Accepted);
    }

    /// ADR 0007, from the other side. The rule is about *values*, so `/opt/mylib` passes and a
    /// path inside the bundle does not — and the same variable name does both.
    #[test]
    fn a_value_inside_the_bundle_is_refused_and_one_outside_it_is_not() {
        let appdir = Some("/tmp/.mount_cideAAA");
        assert_eq!(
            env_verdict("LD_LIBRARY_PATH", "/opt/mylib", appdir),
            Verdict::Accepted,
            "adding a library path of one's own is legitimate and harmless"
        );
        assert!(
            env_verdict("LD_LIBRARY_PATH", "/tmp/.mount_cideAAA/usr/lib", appdir).is_refused(),
            "this is the value AppRun sets and the one that kills every stdio MCP server"
        );
        assert!(
            env_verdict("PYTHONHOME", "/tmp/.mount_cideAAA/usr", appdir).is_refused(),
            "the fatal one: a prefix with no stdlib in it"
        );
        assert!(
            env_verdict("SOMETHING_NEW", "/tmp/.mount_cideAAA/usr/share", appdir).is_refused(),
            "a name nobody wrote down is caught by the value rule, which is why this is not a \
             name list"
        );
        // And the boundary is a whole path component, as `under` promises.
        assert_eq!(
            env_verdict("LD_LIBRARY_PATH", "/tmp/.mount_cideAAAA/usr/lib", appdir),
            Verdict::Accepted,
            "two AppImages one random suffix apart are ordinary"
        );
    }

    /// Every development run, every `.deb`, every Flatpak. Tested through the pure function
    /// because the alternative is mutating this process's environment, which edition 2024 makes
    /// `unsafe` for exactly the reason a test suite cannot accept.
    #[test]
    fn the_bundle_rule_is_inert_when_cide_is_not_bundled() {
        assert_eq!(
            env_verdict("LD_LIBRARY_PATH", "/tmp/.mount_cideAAA/usr/lib", None),
            Verdict::Accepted,
            "with no APPDIR there is no bundle to point inside of"
        );
    }

    #[test]
    fn a_blank_row_is_not_an_error_and_contributes_nothing() {
        let (kept, notes) = user_env(&cli(&[], &[("", "value"), ("  ", "")]), None);
        assert!(kept.is_empty());
        assert!(
            notes.is_empty(),
            "a freshly added row the user has not typed into yet is not a refusal"
        );
    }

    #[test]
    fn duplicates_survive_in_order_because_the_last_one_is_what_the_child_gets() {
        let (kept, _) = user_env(&cli(&[], &[("A", "1"), ("A", "2")]), None);
        assert_eq!(
            kept,
            [
                ("A".to_string(), Some("1".to_string())),
                ("A".to_string(), Some("2".to_string()))
            ],
            "SpawnSpec applies these in order, so collapsing them here would make the readout \
             disagree with the child"
        );
    }

    #[test]
    fn nothing_here_ever_removes_a_variable() {
        let (kept, _) = user_env(&cli(&[], &[("EMPTY", "")]), None);
        assert_eq!(
            kept,
            [("EMPTY".to_string(), Some(String::new()))],
            "an empty value sets the variable to the empty string; there is deliberately no way \
             to spell `remove`, because that control would let a user delete the \
             ANTHROPIC_API_KEY their own login environment carries"
        );
        assert!(
            user_env(&cli(&[], &[("A", "1"), ("B", "")]), None)
                .0
                .iter()
                .all(|(_, value)| value.is_some())
        );
    }

    #[test]
    fn a_blank_binary_is_a_problem_of_its_own() {
        assert_eq!(resolve(""), Err(BinaryProblem::Blank));
        assert_eq!(resolve("   "), Err(BinaryProblem::Blank));
    }

    #[test]
    fn a_path_that_is_not_executable_is_refused_and_a_bare_name_is_looked_up() {
        let dir = std::env::temp_dir().join(format!("cide-claude-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not-a-program");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();

        assert_eq!(
            resolve(file.to_str().unwrap()),
            Err(BinaryProblem::NotExecutable { path: file.clone() })
        );
        assert!(
            matches!(
                resolve(dir.to_str().unwrap()),
                Err(BinaryProblem::NotExecutable { .. })
            ),
            "a directory is not a program, and `is_executable` already answers false for one"
        );
        assert!(matches!(
            resolve("cide-definitely-not-a-real-binary"),
            Err(BinaryProblem::NotOnPath { .. })
        ));
        // `/bin/sh` exists and is executable on every machine this ever runs on.
        assert_eq!(resolve("/bin/sh"), Ok(PathBuf::from("/bin/sh")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The message is the whole user-facing product of a bad binary, and an empty one would be
    /// a pane that dies with a blank transcript.
    #[test]
    fn every_binary_problem_says_something_and_names_where_to_fix_it() {
        for problem in [
            BinaryProblem::Blank,
            BinaryProblem::NotOnPath {
                name: "claude".into(),
            },
            BinaryProblem::NotExecutable {
                path: PathBuf::from("/opt/claude"),
            },
        ] {
            let message = problem.message();
            assert!(message.len() > 40, "{message}");
            assert!(
                message.contains("Settings"),
                "a refusal that does not say where to correct it is a dead end: {message}"
            );
        }
    }

    /// The substitution has to be decided from what the frontend asked for, never from the
    /// resolved program's file name — which is a version number on this machine.
    #[test]
    fn only_a_claude_pane_gets_the_configured_binary() {
        assert_eq!(
            program_for(true, "/opt/claude-2.1", Path::new("claude")),
            PathBuf::from("/opt/claude-2.1")
        );
        assert_eq!(
            program_for(false, "/opt/claude-2.1", Path::new("/bin/bash")),
            PathBuf::from("/bin/bash"),
            "a shell pane is the user's shell and this screen is not about it"
        );
    }

    /// A refusal with no sentence is a refusal the user cannot act on, and the tables are the
    /// only place those sentences exist.
    #[test]
    fn every_table_entry_carries_a_sentence_and_a_distinct_name() {
        let mut names: Vec<&str> = Vec::new();
        for entry in REFUSED_ARGS.iter().chain(WARNED_ARGS) {
            assert!(entry.flag.starts_with("--"), "{}", entry.flag);
            assert!(entry.reason.len() > 40, "{}", entry.flag);
            names.push(entry.flag);
            names.extend(entry.aliases);
        }
        for (name, reason) in REFUSED_ENV.iter().chain(WARNED_ENV) {
            assert!(reason.len() > 40, "{name}");
            names.push(name);
        }
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            before,
            names.len(),
            "a name in two tables has two verdicts and whichever is consulted first wins in \
             silence"
        );
    }

    /// The tables are read out of this file by `ui/scripts/check-claude-cli.mjs`, which needs
    /// them to stay parseable. Asserted here as well so the failure arrives on the side that
    /// broke it.
    #[test]
    fn the_tables_are_not_empty() {
        assert!(REFUSED_ARGS.len() >= 7);
        assert!(REFUSED_ENV.len() >= 10);
        assert!(!WARNED_ARGS.is_empty());
        assert!(!WARNED_ENV.is_empty());
    }
}
