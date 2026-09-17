//! What a Docker daemon is running, and what it holds. (M41 — ADR 0013)
//!
//! # Why this speaks the Engine API and does not run `docker`
//!
//! ADR 0012 argued the other way for OpenSpec, and the difference is worth stating because the
//! two look alike from a distance. `openspec --json` is an adapter its authors maintain: every
//! command emits a document, and reimplementing their markdown would have been a second parser
//! that drifts. Docker's CLI is not that. `docker ps --format json` flattens ports and mounts
//! into *display strings* built for a terminal, `docker logs` through a pipe loses the
//! stdout/stderr split the daemon actually sends, and `docker inspect` — the one faithful
//! surface — is the raw API object with a CLI in front of it. Shelling out would mean parsing a
//! human interface to recover a machine one that was there all along.
//!
//! So: HTTP/1.1 to the daemon, over whichever socket [`connect`] resolves. The cost is a
//! dependency on API compatibility, and it is paid by negotiating rather than assuming — ask an
//! unversioned `GET /version`, then pin the lower of what the server offers and what cide was
//! built against.
//!
//! # The one exception, and it is a real one
//!
//! **There is no Engine API for compose.** `docker compose up` is a CLI plugin; the daemon has
//! never heard of a stack. What the API does give is labels — every compose-managed container
//! carries `com.docker.compose.project`, `.service` and `.config_files` — so *grouping* is pure
//! API and only *acting* needs the plugin. That lane is quarantined in `compose.rs` behind its
//! own discovery ladder, and when the plugin is missing the stacks still list: only the buttons
//! go quiet, with a sentence.
//!
//! # Threads out, tokio in
//!
//! The client is async underneath and this crate's public API is **blocking**. A private runtime
//! lives in the handle; streams hand back a `crossbeam-channel` receiver. That is not a
//! preference — it is what lets `cmd/docker.rs` call this from `spawn_blocking` like every other
//! domain crate, and what keeps tokio from becoming a fact the rest of the workspace has to know.
//!
//! # No tauri
//!
//! `cide-headless docker` is the standing proof, the way `cide-headless tree` is for the
//! workspace. Everything crossing this crate's boundary is a `cide_ipc::docker` DTO; the shapes
//! the Engine API emits live in `model.rs`, `Deserialize`-only, so an upstream change lands here
//! and nowhere else.

mod api;
pub mod compose;
pub mod connect;
pub mod detail;
pub mod files;
pub mod session;
pub mod stream;

/// Anything that can go wrong between asking and answering.
///
/// Tagged rather than prose, `cide_spec::SpecError`'s reason: each caller phrases its own
/// sentence, and the panel turns every one of these into a *screen* rather than an error — see
/// `cide_ipc::docker::DockerBoard`, whose `Unusable` arm exists because a panel handed an `Err`
/// has nothing to draw and no way to say what to do next.
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    /// No daemon could be chosen. Carries the ladder's own sentence.
    #[error("{0}")]
    NoEndpoint(String),
    /// An endpoint was chosen and the daemon did not answer.
    #[error("{0}")]
    Unreachable(String),
    /// The daemon answered and cide could not read what it said.
    #[error("cide could not read the daemon's answer to {what}: {detail}")]
    Unreadable { what: String, detail: String },
    /// The daemon answered with a refusal, in its own words.
    #[error("{0}")]
    Refused(String),
}

impl From<connect::Refusal> for DockerError {
    fn from(refusal: connect::Refusal) -> Self {
        Self::NoEndpoint(refusal.sentence())
    }
}

/// A connection to one daemon, and the runtime that drives it.
///
/// # Blocking on the outside
///
/// Every method here blocks. The async client and its runtime are private, so `cmd/docker.rs`
/// calls this from `spawn_blocking` exactly as `cmd/spec.rs` calls `cide-spec`, and no other
/// crate in the workspace acquires an opinion about tokio.
///
/// # Why the runtime is multi-threaded, with two workers
///
/// The board's own traffic is one request at a time waiting on a socket, and a current-thread
/// runtime serves that perfectly — which is what this was until M42. Streams changed the
/// requirement rather than the volume: an exec pane and a log follow are **pump tasks that must
/// run while nobody is blocked on them**, and a current-thread runtime only advances a future
/// while someone is inside `block_on`. A spawned pump would sit still between board reads, which
/// is to say a container's output would arrive in bursts whenever something else happened to ask
/// the daemon a question.
///
/// Two workers rather than the default (one per core) because the work is entirely socket-bound
/// and the count is a claim about *concurrent progress*, not throughput: one pump plus one
/// request in flight is the shape, and a sixteen-worker pool on a workstation would be fourteen
/// idle threads per connection.
pub struct Docker {
    runtime: tokio::runtime::Runtime,
    client: bollard::Docker,
    endpoint: connect::Endpoint,
    context: Option<String>,
    contexts: Vec<connect::Context>,
}

/// How long any one request may take before cide gives up on it.
///
/// Generous, and it has to be: `remove` on a large container and `stop` on one that ignores
/// SIGTERM both spend real time in the daemon, and a deadline that fired first would report a
/// failure for an operation that then succeeds — the worst answer available, because the panel
/// would refresh to show it had worked.
const TIMEOUT: u64 = 120;

impl Docker {
    /// Resolve an endpoint and connect to it.
    ///
    /// `endpoint` overrides the ladder — that is the connection switcher, which names a context's
    /// endpoint directly. `None` runs [`connect::find`].
    pub fn open(endpoint: Option<connect::Endpoint>) -> Result<Self, DockerError> {
        let probes = connect::probe();
        let contexts = probes.contexts.clone();
        let endpoint = match endpoint {
            Some(endpoint) => endpoint,
            None => connect::ladder(probes)?,
        };
        // The name is looked up rather than passed in, so that it is right however the endpoint
        // was chosen: an override that happens to point at a context's socket is still that
        // context, and a context switched away from under a running cide stops matching.
        let context = contexts
            .iter()
            .find(|c| c.endpoint == endpoint)
            .map(|c| c.name.clone());

        let client = match &endpoint {
            connect::Endpoint::Unix(path) => bollard::Docker::connect_with_socket(
                &path.to_string_lossy(),
                TIMEOUT,
                bollard::API_DEFAULT_VERSION,
            ),
            // TLS is not compiled in — see the root manifest's `bollard` entry — and connecting
            // in the clear to an endpoint the user spelled `https://` would send a client
            // certificate's worth of trust to a plaintext port. Refuse by name instead.
            connect::Endpoint::Tcp { tls: true, .. } => {
                return Err(DockerError::NoEndpoint(format!(
                    "`{}` is a TLS endpoint, and this build of cide has no TLS support for \
                     Docker. cide has not connected in the clear. Use a unix socket, or an \
                     `ssh` tunnel with {} pointing at its local end.",
                    endpoint.as_url(),
                    connect::OVERRIDE_ENV
                )));
            }
            connect::Endpoint::Tcp { .. } => bollard::Docker::connect_with_http(
                &endpoint.as_url(),
                TIMEOUT,
                bollard::API_DEFAULT_VERSION,
            ),
        }
        .map_err(|error| {
            DockerError::Unreachable(format!(
                "cide could not open a connection to `{}`: {error}",
                endpoint.as_url()
            ))
        })?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cide-docker")
            .enable_all()
            .build()
            .map_err(|error| {
                DockerError::Unreachable(format!("cide could not start a Docker worker: {error}"))
            })?;

        // Negotiation, and it is the first request rather than a lazy one on purpose: it is also
        // the reachability check. A daemon that is not up fails *here*, with the endpoint named,
        // instead of failing later inside whichever call the user happened to make first.
        let client = runtime
            .block_on(client.negotiate_version())
            .map_err(|error| DockerError::Unreachable(reaching(&endpoint, &error.to_string())))?;

        Ok(Self {
            runtime,
            client,
            endpoint,
            context,
            contexts,
        })
    }

    /// The endpoint this connection is to.
    #[must_use]
    pub fn endpoint(&self) -> &connect::Endpoint {
        &self.endpoint
    }

    /// The context store, as rows. Available whether or not the daemon answers — it is read from
    /// `~/.docker` and is what keeps the switcher usable after a failed switch.
    #[must_use]
    pub fn contexts(&self) -> Vec<cide_ipc::docker::ContextRow> {
        api::context_rows(&self.contexts)
    }

    /// Which named context this connection is to, if any.
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// The client and the runtime, for the streaming half in [`session`].
    ///
    /// `pub(crate)` and not public: bollard is a dependency of exactly one crate and must never
    /// be re-exported — the root manifest's rule, and the whole reason a bump stays a diff
    /// inside this crate.
    pub(crate) fn parts(&self) -> (&bollard::Docker, &tokio::runtime::Runtime) {
        (&self.client, &self.runtime)
    }

    /// One coherent read of the daemon: version, containers, images, and the context store.
    pub fn snapshot(&self) -> Result<cide_ipc::docker::DockerSnapshot, DockerError> {
        self.runtime.block_on(api::snapshot(
            &self.client,
            &self.endpoint,
            self.context.as_deref(),
            &self.contexts,
            compose::availability(),
        ))
    }

    /// The raw `inspect` document for a container or an image, pretty-printed. (M43)
    ///
    /// # Why the raw JSON and not a modelled shape
    ///
    /// Because `inspect` is the one place where *everything* is worth showing and cide cannot
    /// know in advance which field somebody is looking for. A modelled view would be a second,
    /// lossy copy of a two-hundred-field document that Docker extends every release, and the
    /// field a user is hunting is disproportionately likely to be one of the new ones. The panel
    /// draws rows for the handful that matter; this is the whole truth behind them, in a
    /// read-only editor tab where cide's own search, folding and syntax colouring already work.
    ///
    /// Pretty-printed here rather than in the webview: the daemon sends one long line, and a
    /// 40 KB single line is a document no editor folds and no eye reads.
    pub fn inspect(&self, target: &cide_ipc::docker::InspectTarget) -> Result<String, DockerError> {
        api::inspect(&self.client, &self.runtime, target)
    }

    /// Run a Compose action on a stack. (M43 — the CLI lane, ADR 0013)
    pub fn compose_act(
        &self,
        working_dir: Option<&std::path::Path>,
        project: &str,
        action: cide_ipc::docker::ComposeAction,
        files: &[String],
    ) -> Result<String, DockerError> {
        compose::act(working_dir, project, action, files)
    }

    /// Do one thing to one container.
    pub fn act(
        &self,
        id: &str,
        action: cide_ipc::docker::ContainerAction,
    ) -> Result<(), DockerError> {
        self.runtime.block_on(api::act(&self.client, id, action))
    }

    /// How much disk each volume uses, by name. (M57)
    ///
    /// **Never call this from a board read.** `GET /volumes` carries no size and `/system/df`
    /// walks the filesystem — 8.4 seconds cold on the machine this was measured on. See
    /// [`api::volume_usage`].
    pub fn volume_usage(&self) -> Result<std::collections::HashMap<String, i64>, DockerError> {
        self.runtime.block_on(api::volume_usage(&self.client))
    }

    /// Remove an image, a volume or a network, refusing if anything is using it. (M56)
    ///
    /// See [`api::remove`] for why nothing here forces — the short version is that the daemon's
    /// refusal names what is using the thing, and that sentence is worth more than the removal.
    pub fn remove(&self, target: &cide_ipc::docker::Removable) -> Result<(), DockerError> {
        self.runtime.block_on(api::remove(&self.client, target))
    }
}

/// A live subscription to the daemon's event stream. (M47)
///
/// Dropping it stops the watch. That is the whole interface: the caller holds one per connection
/// and drops it when the connection goes, which is the only lifetime that makes sense — an
/// endpoint switch must not leave the old daemon's events arriving under the new one's name.
pub struct EventWatch {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for EventWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// How long to wait before re-subscribing after the stream ends.
///
/// The stream ends for two reasons and they want the same answer: the daemon restarted, or the
/// connection was dropped. Retrying immediately would spin against a daemon that is down, and
/// waiting long would mean a panel that stays stale for a minute after `colima start`.
const RESUBSCRIBE: std::time::Duration = std::time::Duration::from_secs(3);

impl Docker {
    /// Call `on_change` whenever the daemon reports that something happened. (M47)
    ///
    /// # Why this exists rather than a poll
    ///
    /// Because the alternative is choosing an interval that is either too slow to feel live or
    /// fast enough to keep a socket busy on an idle machine. `GET /events` is the daemon telling
    /// cide, and it arrives in the bursts a file watcher does — `docker compose up` on a
    /// six-service stack emits some forty events in under a second — which is why the callback
    /// goes through `docker_state`'s coalescer rather than reading a board per event.
    ///
    /// # It resubscribes, and that is the point
    ///
    /// The stream ends whenever the daemon restarts, which on a developer's machine is a normal
    /// part of the day. A watch that ended with it would leave the panel silently stale until the
    /// next manual refresh — indistinguishable, from the user's side, from the feature not
    /// working. So the loop reconnects until it is told to stop.
    pub fn watch_events(&self, on_change: impl Fn() + Send + Sync + 'static) -> EventWatch {
        use futures_util::StreamExt as _;
        use std::sync::atomic::Ordering;

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&stop);
        let client = self.client.clone();

        self.runtime.spawn(async move {
            while !flag.load(Ordering::Acquire) {
                let mut stream = client.events(None::<bollard::query_parameters::EventsOptions>);
                while let Some(event) = stream.next().await {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    /*
                     * The *contents* are deliberately ignored, with one exception. Every event
                     * means "the board is out of date", the coalescer turns a burst into one
                     * read, and a filter here would be a second place that has to know which of
                     * Docker's forty event types change what a panel draws.
                     *
                     * The exception is an event with **no type at all**, which is what this
                     * daemon sends on an idle stream roughly once a minute — measured against
                     * colima: four emits exactly 60 seconds apart, each carrying an unchanged
                     * board. Passing it on costs a four-call board read and an emit to every
                     * window, for nothing, for as long as a panel is open. It is not a filter on
                     * *which* changes matter; it is a refusal to treat a keepalive as a change.
                     */
                    match event {
                        Ok(message) if message.typ.is_some() => on_change(),
                        Ok(_) => {
                            tracing::trace!(
                                "docker: an event with no type — treated as a keepalive"
                            );
                        }
                        Err(_) => {}
                    }
                }
                // The stream ended. Sleep before resubscribing so a daemon that is down does not
                // become a spin.
                tokio::time::sleep(RESUBSCRIBE).await;
            }
        });

        EventWatch { stop }
    }
}

/// Why a socket could not be reached, in words the user can act on.
///
/// # The `EACCES` arm is the whole reason this exists
///
/// On Linux `/var/run/docker.sock` is `root:docker` and mode 660, so a user who is not in the
/// `docker` group gets *permission denied* on a daemon that is running perfectly. That is the
/// single most common first-run failure on Linux, the remedy is one command, and a message
/// saying only "could not reach the daemon" would send somebody to restart a service that was
/// never down.
///
/// The other arm worth naming is the missing socket: where Docker is installed but not started,
/// the file is simply absent, and "no such file" reads as a cide bug rather than as
/// `systemctl start docker`.
///
/// Matched on the message text because that is all there is — `bollard::errors::Error` wraps the
/// `io::Error` in a variant this crate would have to match exhaustively to reach, and the two
/// spellings checked here (`permission denied`, `os error 13`) are what every libc in use
/// produces.
fn reaching(endpoint: &connect::Endpoint, detail: &str) -> String {
    let url = endpoint.as_url();
    let lowered = detail.to_lowercase();
    let unix = matches!(endpoint, connect::Endpoint::Unix(_));

    if unix && (lowered.contains("permission denied") || lowered.contains("os error 13")) {
        return format!(
            "cide is not allowed to use the Docker socket at `{url}`: {detail}. On Linux that \
             socket belongs to the `docker` group — `sudo usermod -aG docker $USER`, then log out \
             and back in. The daemon itself is running; this is a permission, not a fault."
        );
    }
    if unix
        && (lowered.contains("no such file")
            || lowered.contains("not found")
            || lowered.contains("os error 2"))
    {
        return format!(
            "there is no Docker socket at `{url}`: {detail}. The daemon is probably not started \
             — `systemctl --user start docker` for a rootless install, `sudo systemctl start \
             docker` for the system one, or start Docker Desktop, colima or podman if that is \
             what this machine uses."
        );
    }
    format!("cide could not reach the Docker daemon at `{url}`: {detail}")
}

/// Every Docker context on this machine, as rows.
///
/// A free function because it needs no connection: the store is `~/.docker`, and the one caller
/// that matters is the failure path — a board that could not connect still draws a switcher.
#[must_use]
pub fn context_rows() -> Vec<cide_ipc::docker::ContextRow> {
    api::context_rows(&connect::probe().contexts)
}

/// The whole board, failures included — the shape a panel is built against.
///
/// # Why this returns a board and never an `Err`
///
/// `cide_app::spec_state::read`'s rule, and the reason is the same: a panel that receives an
/// error has nothing to draw and no way to say what to do next, while a board that says the
/// daemon is not running has a sentence and a Retry. Every failure below is a *screen*.
#[must_use]
pub fn board(endpoint: Option<connect::Endpoint>) -> cide_ipc::docker::DockerBoard {
    use cide_ipc::docker::DockerBoard;

    let docker = match Docker::open(endpoint) {
        Ok(docker) => docker,
        // The only failure that is `Absent` rather than `Unusable`: there is no endpoint to
        // name, so there is nothing for the panel to say was tried.
        Err(DockerError::NoEndpoint(hint)) => return DockerBoard::Absent { hint },
        Err(error) => {
            // The connection could not even be opened — a socket that is not there, a daemon
            // that is down. The context store is still readable, so the switcher survives.
            return DockerBoard::Unusable {
                reason: error.to_string(),
                endpoint: String::new(),
                contexts: api::context_rows(&connect::probe().contexts),
                context: None,
            };
        }
    };

    match docker.snapshot() {
        Ok(snapshot) => DockerBoard::Ready(Box::new(snapshot)),
        Err(error) => DockerBoard::Unusable {
            reason: error.to_string(),
            endpoint: docker.endpoint().as_url(),
            contexts: docker.contexts(),
            context: docker.context().map(str::to_string),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use connect::Endpoint;
    use std::path::PathBuf;

    /// The two failures a Linux machine actually produces, and the remedy each one needs.
    ///
    /// Neither can be reached from the machine this was written on, which is precisely why they
    /// are asserted on injected text rather than left to be discovered by a user.
    #[test]
    fn the_linux_socket_failures_name_their_own_remedy() {
        let sock = Endpoint::Unix(PathBuf::from("/var/run/docker.sock"));

        // The single most common first-run failure on Linux: the daemon is fine and the user is
        // not in the `docker` group. A message that said only "could not reach" would send
        // somebody to restart a service that was never down.
        let denied = reaching(&sock, "Permission denied (os error 13)");
        assert!(denied.contains("usermod -aG docker"), "{denied}");
        assert!(denied.contains("not a fault"), "{denied}");

        // Installed but not started. "No such file" reads as a cide bug otherwise.
        let missing = reaching(&sock, "No such file or directory (os error 2)");
        assert!(missing.contains("systemctl"), "{missing}");

        // Anything else keeps the plain sentence, with the endpoint named.
        let other = reaching(&sock, "connection reset by peer");
        assert!(other.contains("/var/run/docker.sock"), "{other}");
        assert!(
            !other.contains("usermod"),
            "a reset is not a permissions problem: {other}"
        );
    }

    /// A TCP endpoint gets neither remedy.
    ///
    /// `usermod -aG docker` is about a file, and a `tcp://` daemon has no socket to own. Advice
    /// that cannot apply is worse than none: it sends the reader to change a group membership
    /// that was never involved.
    #[test]
    fn a_tcp_endpoint_is_not_told_about_the_docker_group() {
        let tcp = Endpoint::Tcp {
            host: "build".into(),
            port: 2375,
            tls: false,
        };
        let denied = reaching(&tcp, "Permission denied (os error 13)");
        assert!(!denied.contains("usermod"), "{denied}");
        assert!(denied.contains("tcp://build:2375"), "{denied}");
    }
}
