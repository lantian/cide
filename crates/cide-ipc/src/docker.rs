//! Docker's wire types: what cide shows of a daemon. (M41 — ADR 0013)
//!
//! # These types are cide's contract, not Docker's
//!
//! Everything here is what cide's frontend is promised. It is **not** the JSON the Engine API
//! emits: those shapes live in `cide_docker::model`, are `Deserialize`-only, and are versioned by
//! Docker rather than by this repository. `cide_ipc::spec`'s argument, one daemon over — a change
//! in Docker's API must be able to land in one crate without touching the wire the panels are
//! built against, and the API object for a container is some two hundred fields deep in places
//! nothing here draws.
//!
//! The translation is lossy in one direction and total in the other: `cide-docker` flattens and
//! drops, and everything that survives is a field a panel draws.
//!
//! # Docker's vocabulary is carried as `String`
//!
//! [`ContainerRow::state`], [`ContainerRow::health`] and [`ImageRow::kind`] are strings and not
//! enums, the rule [`crate::spec::ChangeSummary::status`] already states: a value cide has not
//! heard of must render as itself rather than vanish into an `Unknown` arm. Docker adds states —
//! `restarting`, `removing` and `dead` all arrived after `running` and `exited` — and a panel
//! that showed a container as blank because its daemon was newer than cide would be a bug with
//! no symptom to search for.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What cide can say about a Docker daemon right now.
///
/// Three shapes rather than an empty list, [`crate::SpecBoard`]'s argument applied to a daemon:
/// an empty list cannot distinguish "this machine has no Docker", "Docker is installed and the
/// daemon is not running" and "it is running and there is nothing to show", and those are three
/// different screens with three different next actions.
///
/// # Why the failure arms carry a sentence rather than a code
///
/// Because the sentence is the feature. Every way this can fail — no socket on the ladder, a
/// daemon that is not up, `EACCES` on the socket, a context pointing at a machine that is off —
/// has a *different* remedy, and the crate that discovered which one it was is the only place
/// that knows. A panel handed an error code would have to reimplement that reasoning, and a
/// panel handed a bare `Err` has nothing to draw at all. See `cide_docker::connect::Refusal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DockerBoard {
    /// No endpoint anywhere on the ladder. `hint` is the whole sentence, remedy included.
    Absent { hint: String },
    /// An endpoint was chosen and could not be used. `reason` says which and why; `endpoint` is
    /// the URL that was tried, so a user reading "connection refused" can see *what* refused.
    ///
    /// One arm for several causes, deliberately — `SpecBoard::Unusable`'s reason: splitting it
    /// would put a `match` in the frontend over cases only `cide-docker` can tell apart.
    ///
    /// # Why this carries the context list too
    ///
    /// Because otherwise a failed switch is a **trap**. The switcher is drawn from the board, so
    /// a board that dropped its contexts on failure took away the only control that could undo
    /// the switch: pick a context whose daemon is down, and the panel reports it cannot connect
    /// and simultaneously removes the means of going back. That was reported, and the sentence
    /// was "I'm not able to switch context anymore".
    ///
    /// So the connection facts survive the failure. They are not a property of a *working*
    /// daemon — they are read from `~/.docker`, which is still there when nothing answers.
    Unusable {
        reason: String,
        endpoint: String,
        /// Every context the store holds, so the switcher stays usable. Sorted by name.
        contexts: Vec<ContextRow>,
        /// Which one cide was trying, when the endpoint came from a named context.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        context: Option<String>,
    },
    /// A live daemon.
    Ready(Box<DockerSnapshot>),
}

/// Everything one read of a live daemon produced.
///
/// Boxed inside [`DockerBoard::Ready`] because the failure arms are two strings and this is a
/// hundred containers; an unboxed variant makes every `DockerBoard` that size, including the
/// ones a panel holds while showing an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DockerSnapshot {
    /// The endpoint this snapshot came from, as a `DOCKER_HOST` URL.
    pub endpoint: String,
    /// Which named context is in use, when the endpoint came from one. `None` for an endpoint
    /// from `CIDE_DOCKER_HOST`, `DOCKER_HOST` or a bare socket path — those have no name, and
    /// inventing one ("default") would make the switcher lie about what is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    /// Every context the store holds, for the switcher. Sorted by name.
    pub contexts: Vec<ContextRow>,
    /// The API version actually negotiated — the lower of what the daemon offers and what cide
    /// was built against. Shown in Settings, because it is the first thing to ask about when a
    /// field is empty that should not be.
    pub api_version: String,
    /// What the daemon calls itself: `Docker Engine`, `podman`, and so on.
    pub server: String,
    pub containers: Vec<ContainerRow>,
    pub images: Vec<ImageRow>,
    pub volumes: Vec<VolumeRow>,
    pub networks: Vec<NetworkRow>,
    /// Whether `docker compose` is available on this machine, and where it was found.
    ///
    /// # Why the *board* carries this, and not a separate command
    ///
    /// Because the panel needs it in the same frame as the stacks it decides the buttons for.
    /// A stack row is drawn from container labels — pure Engine API — and its `up`/`down`
    /// buttons need a CLI plugin the daemon has never heard of (ADR 0013). Two calls would give
    /// a frame in which the stacks are drawn and their buttons have not yet decided whether they
    /// exist, which is exactly the flicker a user reads as "the button did not appear".
    pub compose: ComposeAvailability,
}

/// Whether stack actions can work, and the sentence when they cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ComposeAvailability {
    /// `docker compose` was found. `version` is what it calls itself.
    Present { version: String },
    /// It was not. `reason` names the remedy — the stacks still list, only the buttons go quiet.
    Absent { reason: String },
}

/// One volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VolumeRow {
    pub name: String,
    pub driver: String,
    /// Where the daemon keeps it. On a remote or virtualised daemon this is a path **on that
    /// machine**, which is worth remembering before offering to open it — cide does not.
    pub mountpoint: String,
    /// The compose project that created it, from `com.docker.compose.project`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// How many containers reference it, or `None` where the daemon did not say.
    ///
    /// `None` and `Some(0)` are different claims and must not draw alike: only the second means
    /// "nothing would break if you removed this".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub in_use_by: Option<i64>,
}

/// One network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NetworkRow {
    pub id: String,
    pub name: String,
    pub driver: String,
    /// `local`, `swarm`, `global`.
    pub scope: String,
    /// The compose project that created it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// The subnets, as CIDR. Empty for a driver that has none — `host` and `none` both do.
    pub subnets: Vec<String>,
}

/// One entry from the Docker context store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContextRow {
    pub name: String,
    pub description: String,
    /// The endpoint as a `DOCKER_HOST` URL.
    pub endpoint: String,
    /// Whether the store's `currentContext` names this one. **Not** whether cide is using it —
    /// an override outranks the store, and a switcher that showed the store's choice as cide's
    /// would be wrong in exactly the case somebody is debugging.
    pub current: bool,
}

/// One container, as a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContainerRow {
    /// The full 64-character id. Never the short form: it is what every later call is addressed
    /// by, and two containers whose ids share twelve characters is rare rather than impossible.
    pub id: String,
    /// The display name, with Docker's leading `/` stripped.
    pub name: String,
    pub image: String,
    /// Docker's own word — `running`, `exited`, `paused`, `restarting`, `created`, `removing`,
    /// `dead`. Carried as written; see the module header.
    pub state: String,
    /// Docker's human status line: `Up 3 hours`, `Exited (0) 2 days ago`. Display only.
    pub status: String,
    /// `healthy`, `unhealthy`, `starting` — or `None` for an image with no healthcheck, which is
    /// most of them. `None` and `"unhealthy"` must never draw alike.
    ///
    /// # Why every `#[ts(optional)]` in this module is paired with `skip_serializing_if`
    ///
    /// Because on its own it makes the **type lie**. `#[ts(optional)]` emits `health?: string`,
    /// which a reader takes to mean the key is absent — and serde still writes `"health": null`.
    /// So a field the TypeScript says is `undefined` arrives as `null`, `=== undefined` is false,
    /// and the next property access throws. That shipped: the Docker panel's first version read
    /// `container.compose === undefined` and died on `null is not an object` the moment a
    /// container had no compose labels, which is most of them.
    ///
    /// The pair is what makes the declared type true on the wire. `adapt.ts` normalises anyway,
    /// for the fields it cannot see into, because a type that can be wrong once can be wrong
    /// again.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub health: Option<String>,
    /// Unix seconds.
    pub created: i64,
    pub ports: Vec<PortRow>,
    /// The compose stack this container belongs to, from `com.docker.compose.project`, with the
    /// service name from `com.docker.compose.service`. `None` for a container nobody composed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub compose: Option<ComposeMembership>,
}

// There is deliberately no `tty` field here, and it is worth saying why because M42 needs the
// answer. Whether a container has a TTY decides whether its log stream is raw or carries
// Docker's 8-byte stdcopy frame header, and reading one as the other paints a screenful of
// control characters. But `GET /containers/json` does not report it — only `inspect` does, which
// is one round trip *per container*, on every read of a board that exists to be refreshed. So
// the answer is fetched once, lazily, when a log or exec stream is opened, where exactly one
// container is in question and one round trip is free.

/// A published port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PortRow {
    /// The port inside the container.
    pub private: u16,
    /// The port on the host, when one is published. `None` is an exposed-but-unpublished port,
    /// which is not the same thing and must not render as `0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub public: Option<u16>,
    /// `tcp`, `udp`, `sctp`.
    pub protocol: String,
    /// The host interface a published port is bound to, when it is not every one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host_ip: Option<String>,
}

/// Which compose stack a container belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ComposeMembership {
    /// `com.docker.compose.project`.
    pub project: String,
    /// `com.docker.compose.service`.
    pub service: String,
    /// `com.docker.compose.project.working_dir`, when the label is present. This is what makes a
    /// stack *actionable*: `docker compose` needs a directory, and the label is the only record
    /// of which one the stack was brought up from.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub working_dir: Option<String>,
    /// `com.docker.compose.project.config_files`, split on `,`.
    pub config_files: Vec<String>,
}

/// One image, as a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageRow {
    /// The `sha256:…` id, in full.
    pub id: String,
    /// Every `repo:tag` this image answers to. **Empty is meaningful**: an untagged image is a
    /// dangling layer, which is the whole basis of a prune, so the panel needs to tell an image
    /// with no tags from one whose tags it failed to read.
    pub tags: Vec<String>,
    /// Unix seconds.
    pub created: i64,
    /// Bytes.
    pub size: i64,
    /// How many containers, running or not, were created from this image. `-1` where the daemon
    /// did not count — Docker's own sentinel, kept rather than flattened to `0`, because `0`
    /// means "safe to remove" and `-1` means "cide does not know".
    pub containers: i64,
    /// Docker's word for what this is: `image`, or a manifest kind on a newer daemon.
    pub kind: String,
}

/// What a lifecycle button asks for.
///
/// One enum rather than one command each: the frontend sends a verb and an id, and the Rust side
/// has a single place where "what does this do to a container that is already stopped" is
/// decided. Seven commands would be seven places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    /// SIGKILL. Distinct from `Stop`, which is SIGTERM and then a timeout.
    Kill,
    /// `force` is implied — a running container is stopped first. The frontend confirms before
    /// sending this; there is no undo and the volumes are deliberately left alone.
    Remove,
}

/// What kind of stream a Docker pane carries. (M42)
///
/// Two, and they are genuinely different rather than two settings of one thing: an exec creates a
/// process inside the container and has an input direction, a follow creates nothing and has
/// none. See `cide_docker::session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DockerStream {
    /// An interactive shell. `docker exec -it`.
    Exec,
    /// `docker logs -f`. Read-only — keystrokes are discarded rather than refused.
    Logs,
}

/// Something the panel can remove, addressed the way the daemon addresses it. (M56)
///
/// # Why this is not [`InspectTarget`], which names the same four things
///
/// Because a **container is deliberately not here**. Removing one already has a road —
/// `docker_container_action`'s `Remove` — and it makes the opposite trade on purpose: it *forces*,
/// because the frontend has already confirmed and a user who said "remove this running container"
/// meant it. Everything in this enum refuses instead. Two roads to one gesture that disagree
/// about whether they force is the split `openPushDialog` was made to prevent, one surface over.
///
/// So removability is a question the **types** answer rather than a runtime refusal on an arm
/// that should never have been reachable. A fifth inspectable kind cannot become silently
/// removable; somebody has to add it here and decide.
///
/// The asymmetry in how they are addressed is the daemon's, not cide's: a volume is named and the
/// other two are identified. Getting it backwards removes nothing and says nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum Removable {
    Image { id: String },
    Volume { name: String },
    Network { id: String },
}

impl Removable {
    /// What to call it in a confirmation, when the caller has nothing better.
    #[must_use]
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Image { .. } => "image",
            Self::Volume { .. } => "volume",
            Self::Network { .. } => "network",
        }
    }
}

/// What a stack button, or a compose file's own action, asks for. (M43, widened in M48)
///
/// # Why the list grew
///
/// It used to stop at `up`, `down` and `restart`, and said so in these words: *there is no
/// `build`, `pull` or `run`, because each of those is a long-running job with output worth
/// watching, which is a pane and not a button — and until there is somewhere to watch it, a
/// button that silently spends four minutes is worse than no button.*
///
/// M48 built that somewhere. A compose action reached from a **file** — the tree's context menu,
/// the editor's gutter, the palette — runs in a terminal pane through
/// [`crate::docker::ComposeRun`], with no deadline, its output on screen and a Ctrl+C that
/// reaches it. So the constraint is lifted rather than argued away, and the three verbs it was
/// keeping out are here.
///
/// `Recreate` is `up` with `--force-recreate`, and it is the one that has to be its own variant
/// rather than a flag: Docker cannot change a running container, so *recreate* is the only verb
/// that answers "I edited this file, apply it" — which is the reason somebody has the file open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ComposeAction {
    Up,
    Down,
    Restart,
    Recreate,
    Build,
    Pull,
}

impl ComposeAction {
    /// Whether this action can be run as a button that waits for it — the panel's road.
    ///
    /// `build` and `pull` answer **false**: both are unbounded (a cold `pull` of a multi-gigabyte
    /// image is normal) and both produce progress that is the whole point of running them, so a
    /// surface with nowhere to show output must not offer them. The panel asks this rather than
    /// carrying its own list, because a fourth long verb added here would otherwise appear as a
    /// button that spends ten minutes saying nothing.
    #[must_use]
    pub fn is_quick(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::Restart | Self::Recreate)
    }

    /// The verb as a user reads it — the menu row, and the pane's title.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Restart => "Restart",
            Self::Recreate => "Recreate",
            Self::Build => "Build",
            Self::Pull => "Pull",
        }
    }
}

/// A compose invocation, resolved and ready to be spawned in a terminal pane. (M48)
///
/// # Why this is a plan and not a command that runs it
///
/// Because what it produces is an **ordinary shell pane**. Everything cide already does for one —
/// the job watcher that lights the pane dot when the run finishes, the structured-log renderer,
/// the proxy environment a `pull` needs, `$EDITOR`, park-across-a-project-switch, detach into a
/// window — lives in `session_spawn`, and a second spawn site in `cmd::docker` would be a second
/// copy of all of it that drifts. So Rust answers the one question the webview cannot
/// (*which binary, and what argv*) and the generic spawn does the rest.
///
/// It is also why there is no deadline here. `cide_docker::compose::act` runs under a ten-minute
/// ceiling because nothing is watching it; a pane is watched by a person, who can read the
/// output and press Ctrl+C.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ComposeRun {
    /// The absolute path to the binary — `docker`, or a standalone `docker-compose`.
    pub program: String,
    /// Every argument, including the `compose` word when the plugin road needs it.
    pub args: Vec<String>,
    /// The directory to run in, which is the compose file's own.
    ///
    /// Load-bearing rather than cosmetic: Compose derives the project name from the working
    /// directory's basename, resolves every relative `build:` context against it, and reads the
    /// `.env` beside it. Running the same file from anywhere else is a different stack.
    pub working_dir: String,
    /// What the pane is called — `compose up : shop`.
    pub title: String,
}

/// What an inspect tab is showing. (M43)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum InspectTarget {
    /// A container, by its full id.
    Container {
        id: String,
    },
    /// An image, by its `sha256:` id.
    Image {
        id: String,
    },
    Volume {
        name: String,
    },
    Network {
        id: String,
    },
}

impl InspectTarget {
    /// What the tab is called.
    ///
    /// The *name* rather than the id, because a tab strip of four `a3f9c1…` tells nobody
    /// anything — and the caller has the name already, since it came from a row that was
    /// drawing it.
    #[must_use]
    pub fn title(&self, name: &str) -> String {
        let what = match self {
            Self::Container { .. } => "container",
            Self::Image { .. } => "image",
            Self::Volume { .. } => "volume",
            Self::Network { .. } => "network",
        };
        format!("{name} : {what}")
    }
}

/// One entry in a container's filesystem. (M44)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContainerEntry {
    /// The base name. Never a path — the browser knows where it is.
    pub name: String,
    /// Whether it can be descended into. Decided by `ls -p`'s trailing slash rather than by the
    /// mode string, because that is the one field BusyBox and GNU spell identically.
    pub directory: bool,
    /// Bytes, where the listing gave a number. `None` for a directory, a device node, and for
    /// any line cide could not read a size out of — which must not render as `0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<i64>,
    /// What a symlink points at, as `ls` printed it. `None` for everything else.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub link: Option<String>,
    /// The mode column, exactly as `ls` printed it — `drwxr-xr-x`. (M48)
    ///
    /// Kept as the string and never parsed into bits: BusyBox, GNU and the ACL/SELinux suffixes
    /// (`+`, `.`) all spell it slightly differently, and a mode cide half-understood would render
    /// as a *wrong* permission set rather than as an unfamiliar one — `ContainerRow::state`'s
    /// rule, applied to the one field where being wrong is a security claim.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<String>,
    /// Owner and group, joined by a colon: `root:root`. (M48)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<String>,
    /// The date column, as `ls` printed it — `Sep 11 09:31` or `Sep 11  2024`. (M48)
    ///
    /// Not a timestamp: `ls` prints the *year* instead of the time once a file is older than six
    /// months, so there is no instant to recover here, and inventing one would date half a
    /// listing to midnight.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub modified: Option<String>,
}

/// What a listing produced, or why it produced nothing. (M44)
///
/// # Why a failure is a shape rather than an error
///
/// `DockerBoard`'s rule, and here it has a specific cause worth naming: **a container built from
/// `scratch` or a distroless base has no shell**, so there is nothing to run `ls` with. That is
/// not a broken cide and not a broken container — it is the ordinary state of a well-built
/// production image, and a tree that answered "empty" would be a lie about a filesystem that is
/// full. See `cide_docker::files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ContainerListing {
    Ready {
        entries: Vec<ContainerEntry>,
    },
    /// No shell, no `ls`, or the daemon refused. `reason` is the whole sentence.
    Unusable {
        reason: String,
    },
}

/// One key/value pair — a label, an environment variable. (M47)
///
/// A pair rather than a `BTreeMap` because the wire order is the order the panel draws, and a map
/// would make that the frontend's problem to re-derive on every render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Pair {
    pub name: String,
    pub value: String,
}

/// One mount a container has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MountRow {
    /// `bind`, `volume`, `tmpfs`.
    pub kind: String,
    /// The volume's name, for a named volume. Empty for a bind.
    pub name: String,
    /// Where it comes from **on the daemon's machine**, which is not this one for a remote or
    /// virtualised daemon. Shown, never opened.
    pub source: String,
    /// Where it lands inside the container.
    pub destination: String,
    pub read_only: bool,
}

/// What the detail pane shows for a container. (M47)
///
/// Structured rather than the raw `inspect` document, which is a different surface — see
/// `TabKind::Docker`. This is the handful of fields somebody looks at; that is the whole truth
/// behind them, and both exist on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContainerDetail {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    /// The entrypoint and command, joined — what the container is actually running.
    pub command: String,
    pub ports: Vec<PortRow>,
    pub mounts: Vec<MountRow>,
    pub env: Vec<Pair>,
    pub labels: Vec<Pair>,
    pub networks: Vec<String>,
    /// `no`, `always`, `unless-stopped`, `on-failure`.
    pub restart_policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub compose: Option<ComposeMembership>,
}

/// What the detail pane shows for an image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImageDetail {
    pub id: String,
    pub tags: Vec<String>,
    pub created: i64,
    pub size: i64,
    pub architecture: String,
    pub os: String,
    /// The entrypoint and command the image declares.
    pub command: String,
    pub env: Vec<Pair>,
    pub labels: Vec<Pair>,
    /// Every port the image exposes, as `80/tcp`.
    pub exposed: Vec<String>,
}

/// What the detail pane shows for a volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VolumeDetail {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub scope: String,
    pub labels: Vec<Pair>,
    pub options: Vec<Pair>,
}

/// What the detail pane shows for a network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NetworkDetail {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub subnets: Vec<String>,
    pub labels: Vec<Pair>,
    /// The containers attached, by name.
    pub attached: Vec<String>,
}

/// The detail pane's content. (M47)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum DockerDetail {
    Container(Box<ContainerDetail>),
    Image(Box<ImageDetail>),
    Volume(Box<VolumeDetail>),
    Network(Box<NetworkDetail>),
    /// It is gone, or the daemon refused. `reason` is the sentence.
    ///
    /// A shape rather than an `Err` for `DockerBoard`'s reason: a container removed between the
    /// row being drawn and the row being clicked is an *ordinary* outcome on this surface, and a
    /// pane that threw would take the panel down with it.
    Missing {
        reason: String,
    },
}

/// What a recreate changes. (M47)
///
/// # Why this is an edit and not a create
///
/// Because Docker cannot change a running container's ports, or its environment, or its mounts.
/// There is no endpoint: the container is immutable once created, and every tool that appears to
/// edit one — IDEA included — removes it and creates another from the same image with the same
/// name. So this carries only the *differences*, and everything else is copied from the
/// container's own `inspect` so that the new one is the old one in every respect nobody edited.
///
/// The consequence that cannot be designed away: **the container id changes**. Anything holding
/// the old one — an exec pane, a log follow — is attached to a container that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContainerEdit {
    /// The published ports the new container should have. Replaces the old set entirely — an
    /// empty list means "publish nothing", which is a thing somebody may well want.
    pub ports: Vec<PortRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absent optional is an **absent key**, not `null`. (M46)
    ///
    /// # The bug this pins
    ///
    /// `#[ts(optional)]` changes the *emitted TypeScript* — `compose?: ComposeMembership` — and
    /// nothing about what serde writes. So without `skip_serializing_if` the wire carries
    /// `"compose": null` while the type promises the key is absent, `=== undefined` is false in
    /// the frontend, and the next property access throws. That shipped: the Docker panel died on
    /// `null is not an object (evaluating 'c.compose.project')` for any container with no compose
    /// labels, which is most of them.
    ///
    /// Asserted over the whole row rather than one field, because the failure is per-field and a
    /// test naming `compose` alone would pass while `health` was still wrong.
    #[test]
    fn an_absent_optional_is_an_absent_key_and_never_null() {
        let row = ContainerRow {
            id: "a".repeat(64),
            name: "solo".into(),
            image: "alpine".into(),
            state: "running".into(),
            status: "Up".into(),
            health: None,
            created: 0,
            ports: vec![PortRow {
                private: 80,
                public: None,
                protocol: "tcp".into(),
                host_ip: None,
            }],
            compose: None,
        };
        let json = serde_json::to_string(&row).expect("serialise");
        assert!(
            !json.contains("null"),
            "no optional may reach the wire as null: {json}"
        );
        for key in ["compose", "health", "public", "hostIp"] {
            assert!(
                !json.contains(&format!("\"{key}\"")),
                "`{key}` is absent when it has no value, not present-and-null: {json}"
            );
        }

        // And the mirror: a value that *is* there still arrives, so the guard above cannot be
        // satisfied by never writing the field at all.
        let row = ContainerRow {
            health: Some("healthy".into()),
            compose: Some(ComposeMembership {
                project: "shop".into(),
                service: "db".into(),
                working_dir: None,
                config_files: vec![],
            }),
            ..row
        };
        let json = serde_json::to_string(&row).expect("serialise");
        assert!(json.contains(r#""health":"healthy""#), "{json}");
        assert!(json.contains(r#""project":"shop""#), "{json}");
        assert!(
            !json.contains("workingDir"),
            "including one nested inside a present value: {json}"
        );
    }

    /// The same claim for the other rows, which have their own optionals.
    #[test]
    fn the_other_rows_carry_no_nulls_either() {
        let volume = VolumeRow {
            name: "v".into(),
            driver: "local".into(),
            mountpoint: "/v".into(),
            project: None,
            in_use_by: None,
        };
        let network = NetworkRow {
            id: "n".into(),
            name: "bridge".into(),
            driver: "bridge".into(),
            scope: "local".into(),
            project: None,
            subnets: vec![],
        };
        let entry = ContainerEntry {
            name: "f".into(),
            directory: false,
            size: None,
            link: None,
            mode: None,
            owner: None,
            modified: None,
        };
        for json in [
            serde_json::to_string(&volume).expect("volume"),
            serde_json::to_string(&network).expect("network"),
            serde_json::to_string(&entry).expect("entry"),
        ] {
            assert!(!json.contains("null"), "{json}");
        }
    }
}
