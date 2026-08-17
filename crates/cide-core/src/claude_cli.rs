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

use cide_ipc::ClaudeCli;

use crate::child_env::EnvChange;

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
/// release can retract; this list is on it, and `cide-claude/tests/real_cli_args.rs` is the
/// `#[ignore]`d check that puts these shapes in front of the installed binary. A flag that is
/// renamed upstream stops being refused, which degrades to today's behaviour — the user gets
/// what they asked for and the pane breaks — rather than to a refusal of something harmless.
pub const REFUSED_ARGS: &[RefusedArg] = &[
    RefusedArg {
        flag: "--session-id",
        aliases: &[],
        takes_value: true,
        reason: "cide passes --session-id itself: the uuid *is* the pane's SessionId, and the \
                 hooks report against it. A second one makes every hook frame name a session \
                 this process has never heard of — no token figures, no busy-versus-idle close \
                 confirm — and writes an id into workspace.json with no transcript behind it.",
    },
    RefusedArg {
        flag: "--resume",
        aliases: &["-r"],
        takes_value: true,
        reason: "cide passes --resume itself when a restored pane continues its conversation. A \
                 second one resumes something else under a pane bound to this one, and the \
                 duplicate-session guard keys on cide having chosen it.",
    },
    RefusedArg {
        flag: "--fork-session",
        aliases: &[],
        takes_value: false,
        reason: "Only legal beside --session-id or --resume, both of which cide owns. A stray \
                 one changes which conversation the pane *is*, and cide's Split and fork \
                 gesture is what passes it deliberately.",
    },
    RefusedArg {
        flag: "--continue",
        aliases: &["-c"],
        takes_value: false,
        reason: "The CLI refuses --session-id beside --continue unless --fork-session is also \
                 given, so this does not degrade a feature — every Claude pane fails to start.",
    },
    RefusedArg {
        flag: "--settings",
        aliases: &[],
        takes_value: true,
        reason: "cide passes --settings itself, carrying the inline hook payload that makes the \
                 status line, the token figures and the fast buffer reload work. One of the two \
                 loses, and if yours wins every hook dies with nothing on screen saying so. Put \
                 your own settings in ~/.claude/settings.json, which cide never edits.",
    },
    RefusedArg {
        flag: "--bare",
        aliases: &[],
        takes_value: false,
        reason: "Skips hooks, and makes authentication strictly ANTHROPIC_API_KEY or \
                 apiKeyHelper — OAuth and the keychain are never read. For a Claude Max or Pro \
                 subscriber that is an authentication failure in every pane, with nothing \
                 naming the cause.",
    },
    RefusedArg {
        flag: "--print",
        aliases: &["-p"],
        takes_value: false,
        reason: "Turns an interactive pane into a one-shot that answers and exits. cide has a \
                 headless lane of its own for that — Generate commit message, Explain selection \
                 — and it is not this one.",
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
    pub reason: &'static str,
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
}

impl Plan {
    /// Rows the child never sees. What the log line names, and what the screen strikes out.
    pub fn refusals(&self) -> impl Iterator<Item = &Judged> {
        self.arg_notes
            .iter()
            .chain(&self.env_notes)
            .filter(|judged| judged.verdict.is_refused())
    }
}

/// The verdict on one argument token, ignoring its neighbours.
///
/// Neighbours matter — a refused `--resume` takes the `abc` after it — which is why the whole
/// list goes through [`user_args`] and this answers only about the token itself.
pub fn arg_verdict(token: &str) -> Verdict {
    if let Some(entry) = REFUSED_ARGS.iter().find(|entry| entry.matches(token)) {
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

        let verdict = arg_verdict(token);
        if let Verdict::Refused(reason) = verdict
            && let Some(entry) = REFUSED_ARGS.iter().find(|entry| entry.matches(token))
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
    let (args, arg_notes) = user_args(cli);
    let (env, env_notes) = user_env(cli, appdir);
    Plan {
        args,
        env,
        arg_notes,
        env_notes,
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
        }
    }

    #[test]
    fn the_default_adds_nothing_at_all() {
        let plan = plan(&ClaudeCli::default(), None);
        assert_eq!(plan, Plan::default());
        assert_eq!(ClaudeSettings::default().cli.binary, "claude");
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
                arg_verdict(token).is_refused(),
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
                arg_verdict(token),
                Verdict::Accepted,
                "{token} is not one of cide's own arguments"
            );
        }
    }

    #[test]
    fn the_equals_form_is_the_same_flag() {
        assert!(arg_verdict("--resume=abc").is_refused());
        assert!(arg_verdict("--session-id=1234").is_refused());
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
