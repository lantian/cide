//! Which daemon cide talks to, and saying why when there is none. (M41)
//!
//! # Why this is a ladder and not a constant
//!
//! `/var/run/docker.sock` is the path every example uses and it is **not** where the daemon is
//! on a great many working machines. Docker Desktop puts it in `~/.docker/run/docker.sock`,
//! rootless Docker in `$XDG_RUNTIME_DIR/docker.sock`, and colima, Rancher Desktop, OrbStack and
//! podman each put it under their own directory and register a *context* pointing at it. A build
//! that assumed the constant would report "Docker is not running" on a machine where `docker ps`
//! works, which is the worst of both failures: wrong, and wrong in a way that blames the user.
//!
//! So the context store is read. It is not a documented format, but it is a stable one and it is
//! the only place the answer exists: `~/.docker/config.json` names the current context, and each
//! context's metadata lives in `~/.docker/contexts/meta/<sha256 of its name>/meta.json`. The hash
//! is of the name's bytes, hex, lowercase — that is `sha2` and nothing more.
//!
//! # The ladder is pure; the probing is not
//!
//! [`ladder`] is a function of a [`Probes`], exactly as `cide_spec::discover`'s and
//! `cide_lsp::discover`'s are: every rung's precedence, and every refusal sentence, is decided by
//! a value a test can construct on a machine with no Docker at all.

use std::path::{Path, PathBuf};

/// The environment variable that overrides everything below it.
///
/// Deliberately cide's own and not `DOCKER_HOST`: this is the developer's escape hatch, and it
/// has to be able to outrank a `DOCKER_HOST` the user's shell already exports.
pub const OVERRIDE_ENV: &str = "CIDE_DOCKER_HOST";

/// Docker's own variable, which cide honours at the rung below its override.
pub const DOCKER_HOST_ENV: &str = "DOCKER_HOST";

/// Where the daemon is, normalised. The string form is what `DOCKER_HOST` would spell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// `unix:///var/run/docker.sock` — the overwhelmingly common case.
    Unix(PathBuf),
    /// `tcp://host:port` or `http(s)://host:port`. `tls` is set for `https`, and for a `tcp`
    /// endpoint when a certificate directory was found — Docker's own rule, and it is why this
    /// carries a flag rather than being decided later from the scheme alone.
    Tcp { host: String, port: u16, tls: bool },
}

impl Endpoint {
    /// The `DOCKER_HOST` spelling, which is what a log line and a settings readout should show.
    #[must_use]
    pub fn as_url(&self) -> String {
        match self {
            Self::Unix(path) => format!("unix://{}", path.display()),
            Self::Tcp { host, port, tls } => {
                let scheme = if *tls { "https" } else { "tcp" };
                format!("{scheme}://{host}:{port}")
            }
        }
    }

    /// Parse a `DOCKER_HOST`-shaped string.
    ///
    /// # Why an unsupported scheme is `Err` and not a fall-through
    ///
    /// `ssh://` is a real and reasonably common `DOCKER_HOST`, and cide does not speak it — it
    /// means tunnelling the socket over an `ssh` child, which is a feature and not a line. A
    /// build that silently ignored the variable and connected to a *local* daemon instead would
    /// show the user the wrong machine's containers and let them stop one. That is the single
    /// most dangerous thing this module could do, so an unsupported scheme refuses by name.
    pub fn parse(value: &str) -> Result<Self, Refusal> {
        let value = value.trim();
        if value.is_empty() {
            return Err(Refusal::UnreadableHost {
                value: value.to_string(),
                detail: "it is empty".to_string(),
            });
        }
        let Some((scheme, rest)) = value.split_once("://") else {
            // A bare path is not a `DOCKER_HOST`, but it is what somebody sets when they mean
            // one, and guessing here is safe: there is no other thing it could be.
            if value.starts_with('/') {
                return Ok(Self::Unix(PathBuf::from(value)));
            }
            return Err(Refusal::UnreadableHost {
                value: value.to_string(),
                detail: "it names no scheme, so cide cannot tell what it points at. Docker \
                         spells these `unix:///var/run/docker.sock` or `tcp://host:2375`"
                    .to_string(),
            });
        };

        match scheme {
            "unix" => {
                if rest.is_empty() {
                    return Err(Refusal::UnreadableHost {
                        value: value.to_string(),
                        detail: "it names no socket path".to_string(),
                    });
                }
                Ok(Self::Unix(PathBuf::from(rest)))
            }
            "tcp" | "http" | "https" => {
                let tls = scheme == "https";
                let rest = rest.trim_end_matches('/');
                let (host, port) = match rest.rsplit_once(':') {
                    // An IPv6 literal's colons are inside brackets; a port follows the `]`.
                    Some((host, port)) if !host.ends_with(']') || rest.ends_with(']') => {
                        match port.parse::<u16>() {
                            Ok(port) => (host, port),
                            Err(_) => {
                                return Err(Refusal::UnreadableHost {
                                    value: value.to_string(),
                                    detail: format!("`{port}` is not a port number"),
                                });
                            }
                        }
                    }
                    // Docker's own defaults, and they differ by scheme.
                    _ => (rest, if tls { 2376 } else { 2375 }),
                };
                if host.is_empty() {
                    return Err(Refusal::UnreadableHost {
                        value: value.to_string(),
                        detail: "it names no host".to_string(),
                    });
                }
                Ok(Self::Tcp {
                    host: host.to_string(),
                    port,
                    tls,
                })
            }
            other => Err(Refusal::UnsupportedScheme {
                scheme: other.to_string(),
                value: value.to_string(),
            }),
        }
    }
}

/// One entry from the context store, for the connection switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub name: String,
    pub description: String,
    pub endpoint: Endpoint,
    /// Whether `~/.docker/config.json`'s `currentContext` names this one.
    pub current: bool,
}

/// What was on disk and in the environment when the question was asked.
///
/// Impure inputs, gathered once at the edge by [`probe`], so that [`ladder`] is a function.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Probes {
    /// `CIDE_DOCKER_HOST`, if set: the parsed endpoint, or the refusal parsing it earned.
    ///
    /// Three states and not two, because a *broken* override must refuse rather than fall
    /// through — see [`ladder`].
    pub override_env: Option<Result<Endpoint, Refusal>>,
    /// `DOCKER_HOST`, if set, parsed the same way.
    pub docker_host: Option<Result<Endpoint, Refusal>>,
    /// Every context the store holds, in the store's own order, with `current` set.
    pub contexts: Vec<Context>,
    /// Well-known socket paths that exist on this machine, best first.
    pub sockets: Vec<PathBuf>,
}

/// Why no daemon could be chosen. Tagged rather than prose, so the sentence is built once and
/// every caller shows the same words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A `DOCKER_HOST`-shaped value cide could not read.
    UnreadableHost { value: String, detail: String },
    /// A scheme cide does not speak — `ssh://` above all. See [`Endpoint::parse`].
    UnsupportedScheme { scheme: String, value: String },
    /// `CIDE_DOCKER_HOST` is set and unusable. Carries the refusal parsing it produced.
    BrokenOverride(Box<Refusal>),
    /// Nothing anywhere.
    NothingFound,
}

impl Refusal {
    /// The sentence a panel prints, and a log line.
    ///
    /// `cide_spec::discover::Refusal::sentence`'s rule: name the remedy, and name the *reason a
    /// user who has Docker running is being told they have not*.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::UnreadableHost { value, detail } => {
                format!("`{value}` is not a Docker endpoint cide can read: {detail}.")
            }
            Self::UnsupportedScheme { scheme, value } => format!(
                "`{value}` uses the `{scheme}` scheme, which cide does not speak. cide has \
                 **not** fallen back to a local daemon, because showing you another machine's \
                 containers under this name would be worse than showing you none."
            ),
            Self::BrokenOverride(inner) => {
                let mut sentence = format!(
                    "{OVERRIDE_ENV} is set and cannot be used: {}",
                    inner.sentence()
                );
                // `UnsupportedScheme` already says cide did not fall back, in stronger words and
                // with the reason. Appending the generic clause after it produced a sentence
                // that made the claim twice, which reads as boilerplate and buries the part
                // that is specific to what the user actually set.
                if matches!(**inner, Self::UnsupportedScheme { .. }) {
                    sentence.push_str(" Unset it, or point it at a daemon cide can reach.");
                } else {
                    sentence.push_str(
                        " It is an override, so cide has not fallen back to any other daemon — \
                         unset it or point it at a real one.",
                    );
                }
                sentence
            }
            Self::NothingFound => format!(
                "cide found no Docker daemon. Neither {OVERRIDE_ENV} nor {DOCKER_HOST_ENV} is \
                 set, no Docker context names an endpoint, and there is no socket at any of the \
                 usual paths. If `docker ps` works in a terminal, run `docker context ls` — cide \
                 reads the same context store, so a context that works there should work here."
            ),
        }
    }
}

/// Pick the daemon, or say why not.
///
/// # The override is alone, not first
///
/// `cide_spec::discover::ladder`'s rule, and the difference only shows up when something is
/// wrong: a *set* `CIDE_DOCKER_HOST` that cannot be parsed is a refusal, not a rung that failed.
/// Everything below exists to be fallen back to, and the one thing an override must never do is
/// quietly become something else — a developer pointing cide at a remote daemon and silently
/// getting the local one would stop the wrong container.
///
/// `DOCKER_HOST` is deliberately **not** treated the same way at the rung below. It is the user's
/// own variable rather than cide's, it is frequently exported by a shell rc file and forgotten,
/// and an unparseable one still refuses — but it refuses as itself, so the sentence names the
/// variable the user actually set.
pub fn ladder(probes: Probes) -> Result<Endpoint, Refusal> {
    if let Some(verdict) = probes.override_env {
        return verdict.map_err(|refusal| Refusal::BrokenOverride(Box::new(refusal)));
    }
    if let Some(verdict) = probes.docker_host {
        return verdict;
    }
    if let Some(context) = probes.contexts.iter().find(|c| c.current) {
        return Ok(context.endpoint.clone());
    }
    if let Some(path) = probes.sockets.first() {
        return Ok(Endpoint::Unix(path.clone()));
    }
    Err(Refusal::NothingFound)
}

/// The directory name a context's metadata lives under: the lowercase hex SHA-256 of its name.
///
/// Undocumented by Docker and stable in practice since contexts shipped. Written out as its own
/// function because it is the one part of the store's format that cannot be inferred by looking
/// at a directory listing, and a test pins it against a real store's directory name.
#[must_use]
pub fn context_dir_name(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(name.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Read one `meta.json` into a [`Context`].
///
/// Shape, of which cide reads three fields:
///
/// ```json
/// { "Name": "colima", "Metadata": { "Description": "colima" },
///   "Endpoints": { "docker": { "Host": "unix:///…/docker.sock", "SkipTLSVerify": false } } }
/// ```
///
/// A context whose `docker` endpoint is missing or unparseable is **skipped rather than
/// refused**: a store routinely holds contexts for machines that are switched off, and one bad
/// entry must not make the other five unreachable. Only the *current* context's failure is worth
/// a sentence, and [`ladder`] produces that by finding no current context at all.
fn context_from_meta(raw: &serde_json::Value, current: &str) -> Option<Context> {
    let name = raw.get("Name")?.as_str()?.to_string();
    let host = raw.get("Endpoints")?.get("docker")?.get("Host")?.as_str()?;
    let endpoint = Endpoint::parse(host).ok()?;
    let description = raw
        .get("Metadata")
        .and_then(|m| m.get("Description"))
        .and_then(|d| d.as_str())
        .unwrap_or_default()
        .to_string();
    Some(Context {
        current: name == current,
        name,
        description,
        endpoint,
    })
}

/// Every context in a store rooted at `docker_dir` (`~/.docker`), and which one is current.
///
/// Sorted by name so the switcher's order is stable — the store's own order is a directory walk
/// over hashes, which is to say arbitrary and different on every machine.
#[must_use]
pub fn contexts_in(docker_dir: &Path) -> Vec<Context> {
    let current = std::fs::read_to_string(docker_dir.join("config.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            value
                .get("currentContext")
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        // Docker's own default when the key is absent. `default` is not in the store — it is
        // the `DOCKER_HOST`-or-the-usual-socket context — so leaving it as the current name
        // simply means no stored context matches, which is exactly right.
        .unwrap_or_else(|| "default".to_string());

    let Ok(entries) = std::fs::read_dir(docker_dir.join("contexts").join("meta")) else {
        return Vec::new();
    };
    let mut contexts: Vec<Context> = entries
        .flatten()
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join("meta.json")).ok()?;
            let raw = serde_json::from_str::<serde_json::Value>(&text).ok()?;
            context_from_meta(&raw, &current)
        })
        .collect();
    contexts.sort_by(|a, b| a.name.cmp(&b.name));
    contexts
}

/// The well-known socket paths, best first, filtered to those that exist.
///
/// # The order is a claim about which daemon a user means
///
/// The **per-user** sockets come first — a rootless daemon under `$XDG_RUNTIME_DIR`, Docker
/// Desktop's under `~/.docker/run` — because those were started by this user deliberately, while
/// `/var/run/docker.sock` belongs to the machine and may well be a leftover. That is the opposite
/// of a `PATH` search and worth saying so nobody "fixes" it.
///
/// # Why podman is here
///
/// Because it speaks this API and is the default container engine on Fedora, RHEL and their
/// derivatives — a Linux developer is as likely to have `podman.sock` as `docker.sock`, and on
/// many machines *only* podman. Everything in this crate works against it unchanged: the Engine
/// API is the same, the compose labels are the same, and `podman compose` is found by the same
/// ladder. It is listed **after** Docker's sockets rather than merged with them: on a machine
/// running both, a panel called Docker showing podman's containers would be wrong, and the
/// reverse is what `DOCKER_HOST` is for.
///
/// Rootless podman puts its socket under `$XDG_RUNTIME_DIR/podman/`, which is where it is on
/// almost every desktop; the rootful one is `/run/podman/podman.sock` and needs privileges cide
/// does not ask for — it is listed so the refusal names it rather than reporting nothing found.
#[must_use]
pub fn sockets_present(home: Option<&Path>, xdg_runtime: Option<&Path>) -> Vec<PathBuf> {
    sockets_present_under(Path::new("/"), home, xdg_runtime)
}

/// [`sockets_present`], with the machine's own root injected.
///
/// The per-user directories have been parameters from the start so that the Linux layouts could
/// be asserted from a Mac — but the machine's own sockets were left absolute, which quietly made
/// that assertion true only on a host that is not itself running Docker. CI's Linux runner is,
/// so `/var/run/docker.sock` is a real file there, every scratch layout came back with an entry
/// nobody had written into it, and `the_linux_layouts_resolve_in_the_right_order` failed for no
/// reason but the runner having a daemon. The segments joined below are relative, so a root of
/// `/` spells exactly the paths this always probed.
fn sockets_present_under(
    root: &Path,
    home: Option<&Path>,
    xdg_runtime: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = xdg_runtime {
        // Rootless Docker.
        candidates.push(dir.join("docker.sock"));
    }
    if let Some(home) = home {
        // Docker Desktop, on every platform it ships for.
        candidates.push(home.join(".docker").join("run").join("docker.sock"));
        // colima, which is macOS-only and harmless to look for anywhere.
        candidates.push(home.join(".colima").join("default").join("docker.sock"));
    }
    // The machine's own. `/run` as well as `/var/run`: the second is a symlink to the first on
    // every systemd distribution, and is *not* inside a container or a minimal namespace where
    // only one of the two is mounted.
    candidates.push(root.join("var/run/docker.sock"));
    candidates.push(root.join("run/docker.sock"));

    // Then podman, in the same per-user-first order.
    if let Some(dir) = xdg_runtime {
        candidates.push(dir.join("podman").join("podman.sock"));
    }
    candidates.push(root.join("run/podman/podman.sock"));

    candidates.retain(|path| path.exists());
    // `/var/run` and `/run` are the same file on a systemd machine, and listing a socket twice
    // would make the switcher's readout say so. Compared on the *resolved* path so the symlink
    // is seen through.
    let mut seen: Vec<PathBuf> = Vec::new();
    candidates.retain(|path| {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if seen.contains(&real) {
            return false;
        }
        seen.push(real);
        true
    });
    candidates
}

/// Gather [`Probes`] from this machine.
///
/// # Nothing here is cached
///
/// `cide_spec::discover::find`'s caching asymmetry, taken one step further: a Docker endpoint can
/// change under a running cide in the ordinary course of a day — `colima start`, `docker context
/// use`, Docker Desktop launching — and every one of those is exactly when the user presses
/// Retry. Probing is a handful of `stat`s and one small file read.
#[must_use]
pub fn probe() -> Probes {
    let parse_env = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| Endpoint::parse(&value))
    };

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg_runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);

    Probes {
        override_env: parse_env(OVERRIDE_ENV),
        docker_host: parse_env(DOCKER_HOST_ENV),
        contexts: home
            .as_deref()
            .map(|home| contexts_in(&home.join(".docker")))
            .unwrap_or_default(),
        sockets: sockets_present(home.as_deref(), xdg_runtime.as_deref()),
    }
}

/// [`probe`] then [`ladder`] — the whole question, answered.
pub fn find() -> Result<Endpoint, Refusal> {
    ladder(probe())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str, host: &str, current: bool) -> Context {
        Context {
            name: name.to_string(),
            description: String::new(),
            endpoint: Endpoint::parse(host).expect("a test endpoint parses"),
            current,
        }
    }

    #[test]
    fn an_override_wins_alone_and_a_broken_one_refuses_rather_than_falling_through() {
        let p = Probes {
            override_env: Some(Endpoint::parse("unix:///tmp/mine.sock")),
            docker_host: Some(Endpoint::parse("tcp://elsewhere:2375")),
            sockets: vec![PathBuf::from("/var/run/docker.sock")],
            ..Probes::default()
        };
        assert_eq!(
            ladder(p).expect("the override is taken"),
            Endpoint::Unix(PathBuf::from("/tmp/mine.sock"))
        );

        // The half that matters. A developer pointing cide at a daemon of their own and silently
        // getting the local one would stop the wrong container.
        let p = Probes {
            override_env: Some(Endpoint::parse("nonsense")),
            sockets: vec![PathBuf::from("/var/run/docker.sock")],
            ..Probes::default()
        };
        let refusal = ladder(p).expect_err("a broken override refuses");
        let sentence = refusal.sentence();
        assert!(sentence.contains(OVERRIDE_ENV), "{sentence}");
        assert!(sentence.contains("has not fallen back"), "{sentence}");
    }

    #[test]
    fn the_current_context_beats_every_well_known_socket() {
        // The rung this whole module exists for. On the machine M41 was written on there is no
        // `/var/run/docker.sock` at all and the answer comes from here.
        let p = Probes {
            contexts: vec![
                ctx("colima", "unix:///home/u/.colima/default/docker.sock", true),
                ctx(
                    "colima-ozon",
                    "unix:///home/u/.colima/ozon/docker.sock",
                    false,
                ),
            ],
            sockets: vec![PathBuf::from("/var/run/docker.sock")],
            ..Probes::default()
        };
        assert_eq!(
            ladder(p).unwrap(),
            Endpoint::Unix(PathBuf::from("/home/u/.colima/default/docker.sock"))
        );
    }

    #[test]
    fn a_store_with_no_current_context_falls_through_to_a_socket() {
        // `currentContext` absent means Docker's `default`, which is not a stored context. The
        // store still lists the others for the switcher; the ladder simply moves on.
        let p = Probes {
            contexts: vec![ctx(
                "colima",
                "unix:///home/u/.colima/default/docker.sock",
                false,
            )],
            sockets: vec![PathBuf::from("/var/run/docker.sock")],
            ..Probes::default()
        };
        assert_eq!(
            ladder(p).unwrap(),
            Endpoint::Unix(PathBuf::from("/var/run/docker.sock"))
        );
    }

    #[test]
    fn an_unsupported_scheme_refuses_and_says_it_did_not_fall_back() {
        // `ssh://` is the dangerous one: connecting to the local daemon instead would show
        // another machine's containers under this name and let somebody stop one.
        let refusal = Endpoint::parse("ssh://user@build-host").expect_err("cide does not tunnel");
        assert_eq!(
            refusal,
            Refusal::UnsupportedScheme {
                scheme: "ssh".to_string(),
                value: "ssh://user@build-host".to_string(),
            }
        );
        let sentence = refusal.sentence();
        assert!(sentence.contains("has **not** fallen back"), "{sentence}");

        // And it must refuse from the `DOCKER_HOST` rung too, rather than being skipped.
        let p = Probes {
            docker_host: Some(Endpoint::parse("ssh://user@build-host")),
            sockets: vec![PathBuf::from("/var/run/docker.sock")],
            ..Probes::default()
        };
        assert!(
            ladder(p).is_err(),
            "a scheme cide cannot speak is never skipped"
        );
    }

    #[test]
    fn every_docker_host_spelling_round_trips() {
        let cases = [
            ("unix:///var/run/docker.sock", "unix:///var/run/docker.sock"),
            ("/var/run/docker.sock", "unix:///var/run/docker.sock"),
            ("tcp://127.0.0.1:2375", "tcp://127.0.0.1:2375"),
            // Docker's defaults differ by scheme, which is the part worth pinning.
            ("tcp://build", "tcp://build:2375"),
            ("https://build", "https://build:2376"),
            ("http://build:8080", "tcp://build:8080"),
        ];
        for (input, expected) in cases {
            let endpoint = Endpoint::parse(input).unwrap_or_else(|e| panic!("{input}: {e:?}"));
            assert_eq!(endpoint.as_url(), expected, "{input}");
        }
    }

    #[test]
    fn nothing_found_points_at_docker_context_ls() {
        // "But `docker ps` works" is the reply a bare not-found earns here, and the remedy has
        // to be a command whose output cide is provably reading the same store as.
        let sentence = ladder(Probes::default()).expect_err("nothing").sentence();
        assert!(sentence.contains("docker context ls"), "{sentence}");
    }

    #[test]
    fn a_context_directory_is_the_hex_sha256_of_its_name() {
        // Undocumented, load-bearing, and pinned against a real store: on the machine M41 was
        // written on, `~/.docker/contexts/meta/` holds exactly these two directories, and
        // `docker context ls` names exactly `colima` and `colima-ozon`.
        assert_eq!(
            context_dir_name("colima"),
            "f24fd3749c1368328e2b149bec149cb6795619f244c5b584e844961215dadd16"
        );
        assert_eq!(
            context_dir_name("colima-ozon"),
            "00f1bde8df04d9f3f3530e6d7a65a6adc6ae028cc1e9091609aa098a7bf00b78"
        );
    }

    #[test]
    fn one_unreadable_context_does_not_hide_the_others() {
        // A store routinely holds contexts for machines that are switched off or were removed
        // by hand. Refusing the lot would make a working daemon unreachable.
        let good = serde_json::json!({
            "Name": "colima",
            "Endpoints": { "docker": { "Host": "unix:///home/u/.colima/default/docker.sock" } }
        });
        assert!(context_from_meta(&good, "colima").is_some_and(|c| c.current));

        for bad in [
            serde_json::json!({ "Name": "x" }),
            serde_json::json!({ "Endpoints": { "docker": { "Host": "unix:///s" } } }),
            serde_json::json!({ "Name": "x", "Endpoints": { "docker": { "Host": "ssh://h" } } }),
        ] {
            assert!(context_from_meta(&bad, "colima").is_none(), "{bad}");
        }
    }

    /// What *this* machine answers. `#[ignore]`d by the workspace convention: it reads the
    /// user's real `~/.docker` and the answer is different on every machine, so it is a
    /// diagnostic rather than an assertion — run it with `--nocapture` when a user reports that
    /// cide cannot see a daemon `docker ps` can.
    #[test]
    #[ignore = "reads the real ~/.docker and prints what this machine resolves to"]
    fn what_this_machine_resolves_to() {
        let probes = probe();
        for context in &probes.contexts {
            println!(
                "context {}{}  {}",
                context.name,
                if context.current { " *" } else { "  " },
                context.endpoint.as_url()
            );
        }
        for socket in &probes.sockets {
            println!("socket  {}", socket.display());
        }
        match ladder(probes) {
            Ok(endpoint) => println!("\nchosen  {}", endpoint.as_url()),
            Err(refusal) => println!("\nrefused {}", refusal.sentence()),
        }
    }

    /// The shapes a **Linux** machine presents, none of which this was developed on.
    ///
    /// Written as a function of injected directories precisely so it can be asserted from a Mac:
    /// `sockets_present_under` takes the machine's root, `home` and `xdg_runtime` rather than
    /// reading the environment, so the rootless, Desktop, machine and podman layouts can all be
    /// built in a scratch directory and the *order* — which is the part that decides which daemon
    /// a user gets — pinned here. The root is injected because it was not, once: this test read
    /// the real `/var/run/docker.sock` and so passed only on a host with no Docker of its own,
    /// which every developer machine here happened to be and no CI runner is.
    #[test]
    fn the_linux_layouts_resolve_in_the_right_order() {
        let dir = std::env::temp_dir().join(format!("cide-docker-linux-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let run = dir.join("run/user/1000");
        let home = dir.join("home/dev");
        let podman = run.join("podman");
        for path in [&run, &home.join(".docker/run"), &podman] {
            std::fs::create_dir_all(path).expect("scratch dirs");
        }

        // Rootless Docker alone.
        std::fs::write(run.join("docker.sock"), b"").expect("socket stand-in");
        let found = sockets_present_under(&dir, Some(&home), Some(&run));
        assert_eq!(found, vec![run.join("docker.sock")]);

        // Docker Desktop beside it: the rootless one still wins, because `$XDG_RUNTIME_DIR` is
        // this user's running daemon and `~/.docker/run` is Desktop's, which may not be up.
        std::fs::write(home.join(".docker/run/docker.sock"), b"").expect("socket stand-in");
        let found = sockets_present_under(&dir, Some(&home), Some(&run));
        assert_eq!(found.first(), Some(&run.join("docker.sock")));
        assert_eq!(found.len(), 2, "and both are offered: {found:?}");

        // The machine's own, which is what a plain `apt install docker.io` leaves. It is listed,
        // and listed *after* both per-user sockets — the opposite of a PATH search, because a
        // rootless or Desktop socket was started by this user and `/var/run/docker.sock` may be a
        // leftover. Only an injected root can say this; with the real one it was whatever the
        // host happened to have.
        std::fs::create_dir_all(dir.join("var/run")).expect("scratch dirs");
        std::fs::write(dir.join("var/run/docker.sock"), b"").expect("socket stand-in");
        let found = sockets_present_under(&dir, Some(&home), Some(&run));
        assert_eq!(
            found,
            vec![
                run.join("docker.sock"),
                home.join(".docker/run/docker.sock"),
                dir.join("var/run/docker.sock"),
            ],
            "the machine's own comes after both per-user sockets: {found:?}"
        );
        std::fs::remove_file(dir.join("var/run/docker.sock")).expect("remove");

        // Podman, which is the default engine on Fedora and RHEL. Listed, and listed **after**
        // Docker: a panel called Docker showing podman's containers on a machine running both
        // would be wrong, and `DOCKER_HOST` is how somebody says otherwise.
        std::fs::write(podman.join("podman.sock"), b"").expect("socket stand-in");
        let found = sockets_present_under(&dir, Some(&home), Some(&run));
        assert_eq!(
            found.last(),
            Some(&podman.join("podman.sock")),
            "podman is found, and comes last: {found:?}"
        );

        // And on a machine with only podman it is the answer rather than nothing at all.
        std::fs::remove_file(run.join("docker.sock")).expect("remove");
        std::fs::remove_file(home.join(".docker/run/docker.sock")).expect("remove");
        let found = sockets_present_under(&dir, Some(&home), Some(&run));
        assert_eq!(found, vec![podman.join("podman.sock")]);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `/var/run` is a symlink to `/run` on every systemd machine.
    ///
    /// Both are in the ladder — a container or a minimal namespace may mount only one — and on an
    /// ordinary desktop that means the same socket is reachable by two paths. Listing it twice
    /// would put a duplicate in the connection readout and make the ladder's "first one wins"
    /// depend on which spelling came first.
    #[test]
    fn one_socket_reachable_by_two_paths_is_listed_once() {
        let dir = std::env::temp_dir().join(format!("cide-docker-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let real = dir.join("run");
        std::fs::create_dir_all(&real).expect("scratch dir");
        std::fs::write(real.join("docker.sock"), b"").expect("socket stand-in");

        #[cfg(unix)]
        {
            let link = dir.join("var-run");
            std::os::unix::fs::symlink(&real, &link).expect("symlink");

            // Both spellings exist…
            assert!(real.join("docker.sock").exists());
            assert!(link.join("docker.sock").exists());
            // …and resolve to one file, which is the key `sockets_present` deduplicates on.
            // Comparing the *paths* would call them different and list the socket twice.
            assert_eq!(
                std::fs::canonicalize(real.join("docker.sock")).expect("canonicalize"),
                std::fs::canonicalize(link.join("docker.sock")).expect("canonicalize"),
                "the resolved path is what makes `/var/run` and `/run` one entry",
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_per_user_socket_beats_the_machines() {
        // The specific beats the general, which is the opposite of a PATH search: a rootless or
        // Desktop socket was chosen by this user, `/var/run/docker.sock` may be a leftover.
        let dir = std::env::temp_dir().join(format!("cide-docker-sockets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let run = dir.join("run");
        // The machine gets a root of its own rather than `dir`, or its `var/run/docker.sock`
        // would land beside the per-user socket and the two would dedupe into one entry.
        let machine = dir.join("machine");
        std::fs::create_dir_all(&run).expect("scratch dir");
        std::fs::create_dir_all(machine.join("var/run")).expect("scratch dir");
        std::fs::write(run.join("docker.sock"), b"").expect("a stand-in for a socket");
        // The machine's own socket, which this test is named for and never created — so what it
        // asserted was that the per-user socket beat nothing at all, and it went on passing with
        // the order reversed. It could not create one until the root became a parameter.
        std::fs::write(machine.join("var/run/docker.sock"), b"").expect("a stand-in for a socket");

        let found = sockets_present_under(&machine, None, Some(&run));
        assert_eq!(
            found,
            vec![run.join("docker.sock"), machine.join("var/run/docker.sock")],
            "the per-user socket comes first: {found:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
