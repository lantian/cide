//! The proxy variables a child of cide is spawned with — the one rule, for all four spawn
//! sites.
//!
//! # Why this is not in `cmd/session.rs` any more
//!
//! It was, and that was correct while panes were the only children whose proxy anybody had
//! decided. [`cide_ipc::ProxyScope`] makes it three: `$SHELL` panes, `claude` (panes *and*
//! the headless one-shot lane), and cide's own `git push`/`git fetch`. Three spawn sites in
//! three crates now need the same answer to the same question, and the alternative to moving
//! it here was three copies of a rule with six variables, two spellings each, a fallback and
//! an exemption list in it. Every mirror in this repository that was described as "small
//! enough to keep in step by hand" is one this project later had to write a check script for.
//!
//! `cide-core` rather than `cide-ipc` for the usual reason: `cide-ipc` is data, this is
//! behaviour. `cide-core` beside [`crate::child_env`] specifically, because that module
//! already answers the neighbouring question — what a child must *not* inherit from the way
//! cide itself was launched — and the two are applied one after the other at every one of
//! those sites.
//!
//! # The output shape, and why it is a list rather than a `Command`
//!
//! [`ProxyEnv::changes`] hands back [`crate::child_env::EnvChange`]s: `(name, Some(value))`
//! sets, `(name, None)` removes. That is what lets the same answer be folded onto a
//! `cide_pty::SpawnSpec` (which this crate cannot see) and onto a `std::process::Command`
//! (which it can) without either shape being privileged. It is also what makes the whole
//! thing testable without spawning anything.
//!
//! # `None` is not "leave it alone"
//!
//! This is the distinction the whole feature turns on, so it is worth naming twice. An empty
//! change list means *cide touches nothing* — the child inherits whatever this process has,
//! proxy included. A change list of `(name, None)` entries means *cide removes it*, which is
//! the only thing that can promise a child is off a proxy the user's login profile exported.
//! [`ProxyTarget::Untouched`] produces the first; [`ProxyTarget::Direct`] produces the second.

use std::process::Command;

use cide_ipc::{ProxyMode, ProxySettings, ProxyTarget, normalize_proxy_url, redact_proxy_url};

use crate::child_env::EnvChange;

/// The three proxy variables, in the spelling the tooling on this platform expects.
///
/// **Both cases are always written, and it is not belt-and-braces.** curl documents
/// `http_proxy` as lower case *only* — the upper-case form is deliberately ignored because a
/// CGI environment turns an incoming `Proxy:` header into `HTTP_PROXY` — while Go's
/// `httpproxy.FromEnvironment` prefers the upper-case one and much of the Node ecosystem
/// (`proxy-from-env`, which is what axios and friends use) reads lower first and upper
/// second. A pane that sets one spelling proxies half the commands the user types in it, and
/// the other half fail as a connection timeout with nothing anywhere saying why. So each of
/// these names is set — or removed — in both cases, always to the same value.
pub const PROXY_URL_VARS: [&str; 3] = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"];

/// Hosts that must never go through a proxy, whatever the user configured.
///
/// This is not a nicety. cide's IDE integration is an MCP server bound to loopback and found
/// through `CLAUDE_CODE_SSE_PORT`; a proxy that accepts `127.0.0.1:<port>` and forwards it
/// somewhere else takes inline diffs, @-mentions and the editor selection with it, silently,
/// and the user has no reason to connect the two. `cide-hook` talks over a unix socket and is
/// unaffected — this covers the one loopback TCP thing we own.
pub const LOOPBACK_EXEMPT: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// The `NO_PROXY` value: the loopback exemption, then whatever else was asked for.
///
/// Deduplicated case-insensitively so that a user who types `localhost` themselves does not
/// get it twice, and loopback-first so the non-negotiable part is the part you read.
pub fn no_proxy_value(extra: &str) -> String {
    let mut entries: Vec<String> = LOOPBACK_EXEMPT.iter().map(|s| (*s).to_string()).collect();
    for entry in extra.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !entries.iter().any(|e| e.eq_ignore_ascii_case(entry)) {
            entries.push(entry.to_string());
        }
    }
    entries.join(",")
}

/// What cide will do to one child's proxy environment, resolved.
///
/// Built once per spawn from [`ProxySettings`] and the [`ProxyTarget`] for that kind of
/// child, then applied. Holding the resolved answer rather than the settings is what lets
/// `cide-git` take a parameter it can apply without knowing what a proxy mode is.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    changes: Vec<EnvChange>,
    /// The `user:password` prefixes of every URL this will set, for [`Self::scrub_output`].
    ///
    /// Kept apart from `changes` rather than re-derived from it, because the question
    /// "which secrets did cide hand this child" has exactly one honest answer and it is this
    /// list. Empty whenever nothing credentialed is being set, which is almost always.
    secrets: Vec<String>,
}

impl std::fmt::Debug for ProxyEnv {
    /// Hand-written for the same reason [`ProxySettings`]'s is: `changes` holds the proxy
    /// URLs verbatim, password and all, and `tracing::debug!(?env)` anywhere in the app must
    /// not print one. Derived, this type would be the leak that the careful `Debug` on
    /// `ProxySettings` was written to prevent, reintroduced one struct downstream.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyEnv")
            .field("changes", &RedactedChanges(&self.changes))
            // Never the values. The count is the useful part: it answers "is a password
            // involved at all" without being one.
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

struct RedactedChanges<'a>(&'a [EnvChange]);

impl std::fmt::Debug for RedactedChanges<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.iter().map(|(name, value)| {
                (
                    name,
                    value.as_deref().map(redact_proxy_url).unwrap_or_default(),
                )
            }))
            .finish()
    }
}

impl ProxyEnv {
    /// Resolve the settings for one kind of child.
    ///
    /// Pure over `inherited`, which is why every branch below is provable without spawning
    /// anything or touching the process environment. `inherited` answers what *this* process
    /// has, which is what a child would otherwise get; production passes
    /// [`Self::for_target`], which reads `std::env::var`.
    ///
    /// # Inherit vs override
    ///
    /// [`ProxyMode::Manual`] wins outright: every one of the six URL variables is set from
    /// these settings or removed, so a `HTTP_PROXY` in the user's `.bashrc` cannot supply the
    /// half they left blank. A setting that says "this is the proxy" and then silently loses
    /// to something invisible in a shell profile is worse than no setting, because the user
    /// has no way to see which one won.
    ///
    /// [`ProxyMode::Inherit`] is the default and defers completely — with one exception that
    /// is about loopback rather than about proxying. If the inherited environment already
    /// names a proxy, `NO_PROXY` is rewritten to include [`LOOPBACK_EXEMPT`] on top of
    /// whatever it already said. Without that, the common case — a corporate laptop with
    /// `HTTP_PROXY` in the profile and no `NO_PROXY` — is one where cide's headline feature
    /// has never worked and nothing reports it. When nothing is inherited, nothing is touched
    /// at all.
    pub fn resolve(
        proxy: &ProxySettings,
        target: ProxyTarget,
        inherited: impl Fn(&str) -> Option<String>,
    ) -> Self {
        match target {
            // The empty answer, and it is not the same as `Direct`'s. Nothing is set and
            // nothing is removed, so the child gets cide's own environment — proxy included,
            // if cide was launched from a shell that exported one.
            ProxyTarget::Untouched => Self::default(),
            ProxyTarget::Direct => Self {
                changes: Self::scrubbed(),
                secrets: Vec::new(),
            },
            ProxyTarget::Configured => Self::configured(proxy, inherited),
        }
    }

    /// [`Self::resolve`] against this process's own environment.
    pub fn for_target(proxy: &ProxySettings, target: ProxyTarget) -> Self {
        Self::resolve(proxy, target, |name| std::env::var(name).ok())
    }

    fn configured(proxy: &ProxySettings, inherited: impl Fn(&str) -> Option<String>) -> Self {
        match proxy.mode {
            ProxyMode::Inherit => {
                // Each spelling is tested for a *usable* value before the other is consulted,
                // rather than filtering once after the fallback. `HTTP_PROXY=` — set but
                // empty, which is how a profile turns a proxy off without unsetting it, and
                // what `env -u` leaves behind in a few launchers — would otherwise count as
                // present and mask a real `http_proxy` beside it. Both consequences are
                // silent: the loopback exemption is skipped on a machine that does have a
                // proxy, and the `no_proxy` list the user does have is overwritten with one
                // that dropped its entries, since both spellings are written over.
                let named = |name: &str| {
                    let usable = |v: String| (!v.trim().is_empty()).then_some(v);
                    inherited(name)
                        .and_then(usable)
                        .or_else(|| inherited(&name.to_lowercase()).and_then(usable))
                };
                if !PROXY_URL_VARS.iter().any(|v| named(v).is_some()) {
                    // The overwhelmingly common case, and the one where doing anything at all
                    // would be meddling: no proxy anywhere, so no variable is added or
                    // removed.
                    return Self::default();
                }
                let existing = named("NO_PROXY").unwrap_or_default();
                Self {
                    changes: both_cases("NO_PROXY", Some(no_proxy_value(&existing))),
                    secrets: Vec::new(),
                }
            }
            ProxyMode::Manual => {
                let urls = [
                    ("HTTP_PROXY", normalize_proxy_url(&proxy.http)),
                    ("HTTPS_PROXY", proxy.https_url()),
                    ("ALL_PROXY", normalize_proxy_url(&proxy.all)),
                ];
                let secrets = urls
                    .iter()
                    .filter_map(|(_, url)| url.as_deref().and_then(userinfo))
                    .collect();
                let mut changes: Vec<EnvChange> = urls
                    .into_iter()
                    .flat_map(|(name, url)| both_cases(name, url))
                    .collect();
                changes.extend(both_cases(
                    "NO_PROXY",
                    Some(no_proxy_value(&proxy.no_proxy)),
                ));
                Self { changes, secrets }
            }
            ProxyMode::Direct => Self {
                changes: Self::scrubbed(),
                secrets: Vec::new(),
            },
        }
    }

    /// Every proxy name, in both spellings, removed.
    ///
    /// `NO_PROXY` goes too. Leaving an inherited one behind would be harmless but confusing:
    /// a child with no proxy and a long exemption list reads as though something is still
    /// routing.
    fn scrubbed() -> Vec<EnvChange> {
        PROXY_URL_VARS
            .iter()
            .chain(std::iter::once(&"NO_PROXY"))
            .flat_map(|name| both_cases(name, None))
            .collect()
    }

    /// The changes to make, in application order.
    pub fn changes(&self) -> &[EnvChange] {
        &self.changes
    }

    /// True when cide neither sets nor removes anything — [`ProxyTarget::Untouched`], and the
    /// no-proxy-anywhere case of [`ProxyMode::Inherit`].
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Apply to a `std::process::Command`.
    ///
    /// For the children spawned with `std::process` — the `claude` one-shots, `git push`,
    /// `git fetch`. PTY children take the same changes through `SpawnSpec`, folded in
    /// `cide_app::cmd::session`, because this crate cannot see `cide-pty`'s types.
    pub fn apply(&self, command: &mut Command) {
        for (name, value) in &self.changes {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
    }

    /// Remove from `text` any credentials this environment put into the child.
    ///
    /// # Why this exists, and why only now
    ///
    /// `git`'s stderr is shown to the user verbatim — that is the whole reason the binary
    /// route exists, because the remote's own text is the part worth reading. Until this
    /// module, cide never put a proxy URL into `git`'s environment, so the worst that stream
    /// could carry was a proxy the user's own profile had set and could already see.
    /// [`ProxyTarget::Configured`] on `git` changes that: cide now hands `git` a URL that may
    /// carry `user:password@`, and curl is entirely capable of echoing the proxy it failed to
    /// reach back into an error message that then becomes `GitError::Push { output }`, a
    /// toast, and a screenshot in a bug report.
    ///
    /// A substring replacement rather than a parser, on the same argument as
    /// [`cide_ipc::redact_proxy_url`]: this must never fail, and it only has to find strings
    /// it put there itself. It is a *second* line — the first is not putting a password
    /// anywhere it is not needed — and it is cheap enough not to have to choose.
    pub fn scrub_output(&self, text: &str) -> String {
        if self.secrets.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for secret in &self.secrets {
            out = out.replace(secret.as_str(), "***");
        }
        out
    }

    /// One line for a log, with any credentials removed.
    ///
    /// The host survives, because the single most useful thing to have in a log when a child
    /// cannot reach the network is which proxy it was given. `http://user:pass@proxy:3128` is
    /// an ordinary value for that, so the userinfo does not.
    pub fn describe(&self) -> String {
        if self.changes.is_empty() {
            return "untouched".to_string();
        }
        let described: Vec<String> = self
            .changes
            .iter()
            // One spelling in the line. Printing eight entries where four carry the
            // information reads as a bug in the log rather than as the deliberate redundancy
            // it is.
            .filter(|(name, _)| name.chars().all(|c| !c.is_ascii_lowercase()))
            .map(|(name, value)| match value {
                Some(value) => format!("{name}={}", redact_proxy_url(value)),
                None => format!("{name}=<removed>"),
            })
            .collect();
        described.join(" ")
    }
}

/// One change under both spellings of `name`.
fn both_cases(name: &str, value: Option<String>) -> Vec<EnvChange> {
    vec![
        (name.to_string(), value.clone()),
        (name.to_lowercase(), value),
    ]
}

/// The `user:password@` of a URL, if it has one — the exact substring to look for in a
/// child's output.
///
/// Includes the `@` so that a replacement leaves a legible `***@proxy.corp:3128` rather than
/// `***proxy.corp:3128`, and so the match cannot start inside a path.
fn userinfo(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority_end = rest.find('/').unwrap_or(rest.len());
    // `rfind`, matching `redact_proxy_url`'s `rsplit_once`: a password may itself contain an
    // `@`, and stopping at the first one would leave the tail of it in the text.
    let at = rest[..authority_end].rfind('@')?;
    Some(rest[..=at].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use cide_ipc::ProxyScope;

    /// The changes as a map from name to value, for assertions that do not care about order.
    fn resolved(
        proxy: &ProxySettings,
        target: ProxyTarget,
        inherited: &[(&str, &str)],
    ) -> ProxyEnv {
        let inherited: Vec<(String, String)> = inherited
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ProxyEnv::resolve(proxy, target, move |name| {
            inherited
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        })
    }

    fn value_of(env: &ProxyEnv, name: &str) -> Option<Option<String>> {
        env.changes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    fn manual(http: &str, https: &str, all: &str, no_proxy: &str) -> ProxySettings {
        ProxySettings {
            mode: ProxyMode::Manual,
            scope: ProxyScope::default(),
            http: http.into(),
            https: https.into(),
            all: all.into(),
            no_proxy: no_proxy.into(),
        }
    }

    #[test]
    fn the_no_proxy_default_touches_nothing() {
        let env = resolved(&ProxySettings::default(), ProxyTarget::Configured, &[]);
        assert!(env.is_empty(), "{env:?}");
        assert_eq!(env.describe(), "untouched");
    }

    #[test]
    fn a_manual_proxy_sets_each_variable_in_both_cases() {
        let env = resolved(
            &manual(
                "http://proxy.corp:3128",
                "http://tls.corp:3129",
                "socks5://socks.corp:1080",
                "",
            ),
            ProxyTarget::Configured,
            &[],
        );
        for (upper, expected) in [
            ("HTTP_PROXY", "http://proxy.corp:3128"),
            ("HTTPS_PROXY", "http://tls.corp:3129"),
            ("ALL_PROXY", "socks5://socks.corp:1080"),
        ] {
            assert_eq!(value_of(&env, upper), Some(Some(expected.to_string())));
            assert_eq!(
                value_of(&env, &upper.to_lowercase()),
                Some(Some(expected.to_string())),
                "{upper} lower case — curl reads only this spelling of http_proxy"
            );
        }
    }

    /// One proxy typed once reaches both HTTP and HTTPS.
    #[test]
    fn https_defaults_to_the_http_proxy_and_all_proxy_does_not() {
        let env = resolved(
            &manual("proxy.corp:3128", "", "", ""),
            ProxyTarget::Configured,
            &[],
        );
        assert_eq!(
            value_of(&env, "HTTPS_PROXY"),
            Some(Some("http://proxy.corp:3128".into()))
        );
        assert_eq!(
            value_of(&env, "ALL_PROXY"),
            Some(None),
            "ALL_PROXY covers protocols beyond the web; filling it in from the HTTP proxy \
             would change behaviour nobody asked for"
        );
        assert_eq!(value_of(&env, "all_proxy"), Some(None));
    }

    /// Manual mode overrides an inherited proxy rather than merging with it. A user who set
    /// one proxy here and left `ALL_PROXY` empty must not silently get the one their
    /// `.bashrc` exports.
    #[test]
    fn manual_mode_overrides_an_inherited_proxy_rather_than_merging_with_it() {
        let env = resolved(
            &manual("http://proxy.corp:3128", "", "", ""),
            ProxyTarget::Configured,
            &[("ALL_PROXY", "socks5://profile.corp:1080")],
        );
        assert_eq!(value_of(&env, "ALL_PROXY"), Some(None));
    }

    #[test]
    fn loopback_is_exempt_and_cannot_be_configured_away() {
        let env = resolved(
            &manual("http://p:3128", "", "", "corp.internal"),
            ProxyTarget::Configured,
            &[],
        );
        assert_eq!(
            value_of(&env, "no_proxy"),
            Some(Some("localhost,127.0.0.1,::1,corp.internal".into()))
        );
    }

    #[test]
    fn the_exemption_list_does_not_repeat_what_the_user_already_wrote() {
        let env = resolved(
            &manual("http://p:3128", "", "", "LocalHost,corp"),
            ProxyTarget::Configured,
            &[],
        );
        assert_eq!(
            value_of(&env, "NO_PROXY"),
            Some(Some("localhost,127.0.0.1,::1,corp".into()))
        );
    }

    /// The case this exists for: a proxy in the shell profile, no `NO_PROXY`, IDE server on
    /// loopback. cide defers on the proxy itself and still rescues the loopback.
    #[test]
    fn an_inherited_proxy_still_gets_the_loopback_exemption() {
        let env = resolved(
            &ProxySettings::default(),
            ProxyTarget::Configured,
            &[("http_proxy", "http://profile.corp:3128")],
        );
        assert_eq!(
            value_of(&env, "NO_PROXY"),
            Some(Some("localhost,127.0.0.1,::1".into())),
            "the IDE MCP server is on loopback and a proxy that swallows it is silent"
        );
        // And nothing else — the inherited proxy is left exactly as the profile set it.
        assert_eq!(value_of(&env, "HTTP_PROXY"), None);
        assert_eq!(value_of(&env, "http_proxy"), None);
    }

    #[test]
    fn an_inherited_no_proxy_keeps_its_entries() {
        let env = resolved(
            &ProxySettings::default(),
            ProxyTarget::Configured,
            &[
                ("http_proxy", "http://profile.corp:3128"),
                ("no_proxy", "corp.internal"),
            ],
        );
        assert_eq!(
            value_of(&env, "NO_PROXY"),
            Some(Some("localhost,127.0.0.1,::1,corp.internal".into()))
        );
    }

    /// `HTTP_PROXY=` — set but empty — must not hide the `http_proxy` beside it.
    #[test]
    fn an_empty_upper_case_variable_does_not_mask_the_lower_case_one() {
        let env = resolved(
            &ProxySettings::default(),
            ProxyTarget::Configured,
            &[
                ("HTTP_PROXY", ""),
                ("http_proxy", "http://profile.corp:3128"),
                ("NO_PROXY", ""),
                ("no_proxy", "corp.internal"),
            ],
        );
        assert_eq!(
            value_of(&env, "NO_PROXY"),
            Some(Some("localhost,127.0.0.1,::1,corp.internal".into())),
            "the proxy is inherited and the user's own bypass list survives"
        );
    }

    #[test]
    fn direct_scrubs_every_spelling() {
        let env = resolved(
            &ProxySettings {
                mode: ProxyMode::Direct,
                ..manual("http://ignored:1", "", "", "corp")
            },
            ProxyTarget::Configured,
            &[],
        );
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"] {
            assert_eq!(value_of(&env, name), Some(None), "{name}");
            assert_eq!(value_of(&env, &name.to_lowercase()), Some(None), "{name}");
        }
    }

    // --- the scope, which is the new half ---------------------------------------------------

    /// The distinction the whole feature turns on, asserted as two different answers rather
    /// than described in prose.
    ///
    /// `Untouched` produces *no changes at all*: the child inherits cide's environment, proxy
    /// and all. `Direct` produces eight removals. A build that confused the two would leave a
    /// corporate user's `git push` on the proxy while the screen said it was direct.
    #[test]
    fn untouched_is_not_direct_and_the_difference_is_the_whole_point() {
        let proxy = manual("http://proxy.corp:3128", "", "", "");

        let untouched = resolved(&proxy, ProxyTarget::Untouched, &[]);
        assert!(
            untouched.is_empty(),
            "Untouched must set nothing and remove nothing: {untouched:?}"
        );

        let direct = resolved(&proxy, ProxyTarget::Direct, &[]);
        assert!(!direct.is_empty());
        for name in ["HTTP_PROXY", "http_proxy", "NO_PROXY", "no_proxy"] {
            assert_eq!(
                value_of(&direct, name),
                Some(None),
                "Direct has to *remove* {name}, because cide's own environment may carry a \
                 proxy that not-adding-one leaves in place"
            );
        }
    }

    /// `Direct` ignores the mode entirely, including `Manual` with a configured address.
    #[test]
    fn direct_beats_a_configured_address() {
        let env = resolved(
            &manual("http://proxy.corp:3128", "", "", "corp"),
            ProxyTarget::Direct,
            &[("http_proxy", "http://profile.corp:3128")],
        );
        assert_eq!(value_of(&env, "HTTP_PROXY"), Some(None));
        assert_eq!(value_of(&env, "http_proxy"), Some(None));
    }

    /// `Untouched` ignores the mode too — including `Direct`, which is the surprising half.
    ///
    /// Someone reading "No proxy" in Settings might expect it to reach everything. It cannot:
    /// the target column is what says who is being spoken about, and a child that is out of
    /// scope is out of scope for every mode.
    #[test]
    fn untouched_ignores_even_direct_mode() {
        let env = resolved(
            &ProxySettings {
                mode: ProxyMode::Direct,
                ..ProxySettings::default()
            },
            ProxyTarget::Untouched,
            &[("http_proxy", "http://profile.corp:3128")],
        );
        assert!(env.is_empty(), "{env:?}");
    }

    // --- credentials --------------------------------------------------------------------------

    /// A credentialed proxy reaches the child intact — a redacted one would simply be a proxy
    /// that cannot authenticate — and appears nowhere else.
    #[test]
    fn a_credentialed_proxy_reaches_the_child_and_no_log_line() {
        let proxy = manual(
            "http://alice:hunter2@proxy.corp:3128",
            "",
            "socks5://bob:s3cret@socks.corp:1080",
            "",
        );
        let env = resolved(&proxy, ProxyTarget::Configured, &[]);

        assert_eq!(
            value_of(&env, "HTTP_PROXY"),
            Some(Some("http://alice:hunter2@proxy.corp:3128".into())),
            "a redacted value would be a proxy that cannot authenticate"
        );

        for printed in [env.describe(), format!("{env:?}")] {
            assert!(!printed.contains("hunter2"), "password leaked: {printed}");
            assert!(!printed.contains("s3cret"), "password leaked: {printed}");
            assert!(!printed.contains("alice"), "username leaked: {printed}");
            // The host is what makes the line worth having at all.
            assert!(printed.contains("proxy.corp:3128"), "host lost: {printed}");
        }
    }

    /// The new leak this feature could have created: cide now puts a password into `git`'s
    /// environment, and `git`'s stderr is shown to the user verbatim.
    #[test]
    fn a_password_cide_handed_a_child_is_taken_back_out_of_that_childs_output() {
        let env = resolved(
            &manual("http://alice:hunter2@proxy.corp:3128", "", "", ""),
            ProxyTarget::Configured,
            &[],
        );

        // The shape curl actually produces when a CONNECT fails.
        let stderr = "fatal: unable to access 'https://github.com/x/y.git/': Received HTTP \
                      code 407 from proxy after CONNECT (proxy \
                      http://alice:hunter2@proxy.corp:3128)\n";
        let scrubbed = env.scrub_output(stderr);

        assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
        assert!(!scrubbed.contains("alice"), "{scrubbed}");
        assert!(
            scrubbed.contains("proxy.corp:3128"),
            "the host has to survive or the message stops naming the failure: {scrubbed}"
        );
        assert!(
            scrubbed.contains("407"),
            "and the rest of git's message is not ours to edit: {scrubbed}"
        );
    }

    /// The overwhelmingly common case, and the one where a scrub would be pure cost.
    #[test]
    fn output_is_untouched_when_there_was_no_secret_to_hand_over() {
        let env = resolved(
            &manual("http://proxy.corp:3128", "", "", ""),
            ProxyTarget::Configured,
            &[],
        );
        let text = "To github.com:x/y.git\n   ab77c01..91de44a  main -> main\n";
        assert_eq!(env.scrub_output(text), text);

        // And a child cide gave nothing to cannot have been given a secret either.
        let untouched = resolved(
            &manual("http://alice:hunter2@proxy.corp:3128", "", "", ""),
            ProxyTarget::Untouched,
            &[],
        );
        assert_eq!(untouched.scrub_output("hunter2"), "hunter2");
    }

    #[test]
    fn userinfo_stops_at_the_last_at_inside_the_authority() {
        assert_eq!(
            userinfo("http://u:p@ss@proxy.corp:3128").as_deref(),
            Some("u:p@ss@"),
            "a password containing an `@` must be matched whole, or half of it survives the \
             scrub"
        );
        assert_eq!(userinfo("http://proxy.corp:3128"), None);
        assert_eq!(
            userinfo("http://proxy.corp:3128/pac@x"),
            None,
            "an `@` in the path is not a credential"
        );
    }
}
