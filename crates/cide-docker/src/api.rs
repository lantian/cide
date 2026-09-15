//! The Engine API calls, and the translation into cide's own rows. (M41)
//!
//! # Everything Docker says is optional, and cide's rows are not
//!
//! `ContainerSummary`'s every field is an `Option`, including its **id**. That is an artefact of
//! generating a client from an OpenAPI document rather than a statement about the daemon, and
//! carrying it forward would put a `?` on every field of every panel. So the translation is where
//! optionality stops: a container with no id is *skipped* (there is nothing a row could address),
//! and every other missing field takes a documented default. The defaults are chosen so that a
//! missing value never reads as a real one — an unnamed container shows its short id rather than
//! an empty cell, and [`crate::model::ImageRow::containers`] keeps Docker's `-1` sentinel rather
//! than flattening it to `0`, because `0` means "safe to remove".
//!
//! # A state cide has not heard of
//!
//! `bollard-stubs` models the container state as a closed enum with **no `#[serde(other)]`
//! arm**, so a state a future daemon adds does not become an unknown value — it fails the whole
//! list's deserialisation, and a board of a hundred containers becomes one error. Nothing here
//! can prevent that (it is upstream's `Deserialize`), but two things soften it and both are
//! deliberate: cide asks for the state through `Display`, so once parsed it is Docker's own word
//! and travels as a `String` all the way to the panel; and [`crate::DockerError::Unreadable`]
//! names the call, so the failure says *which* request the daemon answered in a way cide could
//! not read rather than reporting a daemon that is down.

use std::collections::HashMap;

use cide_ipc::docker::{
    ComposeAvailability, ComposeMembership, ContainerAction, ContainerRow, ContextRow,
    DockerSnapshot, ImageRow, NetworkRow, PortRow, VolumeRow,
};

use crate::DockerError;
use crate::connect::{Context, Endpoint};

/// The compose labels cide reads. Docker's own namespace, and the only reason a stack is
/// knowable at all — see the crate header's note on there being no Engine API for compose.
const LABEL_PROJECT: &str = "com.docker.compose.project";
const LABEL_SERVICE: &str = "com.docker.compose.service";
const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";
const LABEL_CONFIG_FILES: &str = "com.docker.compose.project.config_files";

/// Read the whole board: version, containers, images.
///
/// One function rather than three commands, because a snapshot must be *coherent*. Three calls
/// the frontend made separately could interleave with a container starting, and the panel would
/// draw a container list and an image list from two different moments — which shows up as a row
/// that references an image not in the list, and is impossible to reproduce.
pub(crate) async fn snapshot(
    docker: &bollard::Docker,
    endpoint: &Endpoint,
    context: Option<&str>,
    contexts: &[Context],
    compose: ComposeAvailability,
) -> Result<DockerSnapshot, DockerError> {
    let version = docker
        .version()
        .await
        .map_err(|error| unreachable(endpoint, &error))?;

    // `all: true` on both, deliberately. A stopped container and a dangling image are the two
    // things this panel most exists to let somebody find and remove; a default listing hides
    // exactly them.
    let containers = docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::default()
                .all(true)
                .build(),
        ))
        .await
        .map_err(|error| unreadable("the container list", &error))?;

    let images = docker
        .list_images(Some(
            bollard::query_parameters::ListImagesOptionsBuilder::default()
                .all(false)
                .build(),
        ))
        .await
        .map_err(|error| unreadable("the image list", &error))?;

    // Volumes and networks are two more list calls in the *same* read, for `snapshot`'s own
    // reason one paragraph up: a panel drawing four lists from four moments shows a volume
    // belonging to a stack whose containers are not in the container list, and it is impossible
    // to reproduce.
    let volumes = docker
        .list_volumes(None::<bollard::query_parameters::ListVolumesOptions>)
        .await
        .map_err(|error| unreadable("the volume list", &error))?;

    let networks = docker
        .list_networks(None::<bollard::query_parameters::ListNetworksOptions>)
        .await
        .map_err(|error| unreadable("the network list", &error))?;

    Ok(DockerSnapshot {
        endpoint: endpoint.as_url(),
        context: context.map(str::to_string),
        contexts: context_rows(contexts),
        api_version: version.api_version.unwrap_or_default(),
        server: version.platform.map(|p| p.name).unwrap_or_else(|| {
            // Podman answers `/version` without a `Platform`. Naming it "Docker Engine" anyway
            // would be a small lie in the one readout somebody consults when a call behaves
            // unlike the documentation.
            "unknown".to_string()
        }),
        containers: sorted_by_recency(
            containers.iter().filter_map(container_row).collect(),
            |row| (row.created, row.id.clone()),
        ),
        images: sorted_by_recency(images.iter().map(image_row).collect(), |row| {
            (row.created, row.id.clone())
        }),
        volumes: sorted_by_name(
            volumes
                .volumes
                .unwrap_or_default()
                .iter()
                .map(volume_row)
                .collect(),
            |row| row.name.clone(),
        ),
        networks: sorted_by_name(networks.iter().filter_map(network_row).collect(), |row| {
            row.name.clone()
        }),
        compose,
    })
}

/// Sort a list the daemon returns in **no order at all**, by name.
///
/// # Why this is a correctness fix and not tidying
///
/// `GET /networks` and `GET /volumes` return their rows in a different order **on every call** —
/// measured against a live daemon, six consecutive reads, six different orders. The panel refetches
/// the whole board on every daemon event, so the Networks and Volumes sections reshuffled
/// themselves several times a minute under the user's pointer. Reported as *"network group rows
/// changing frequently"*, and it is: they were changing on every refresh.
///
/// `docker network ls` does not have this problem because the **CLI** sorts. cide talks to the API
/// (ADR 0013), so the sort is cide's to do — the same conclusion `files::parse_listing` reached
/// about `ls` output, one module over: *the order is the source's and differs between reads*.
///
/// Case-insensitive, because a list mixing `bridge` with `Redis_default` sorted by byte value puts
/// every capital ahead of every lowercase, which reads as no order at all to the person looking at
/// it.
fn sorted_by_name<T, K: FnMut(&T) -> String>(mut rows: Vec<T>, mut key: K) -> Vec<T> {
    rows.sort_by_key(|row| key(row).to_lowercase());
    rows
}

/// Sort a list the daemon returns **newest first**, keeping that order and making it total.
///
/// `GET /containers/json` and `GET /images/json` *are* ordered — by creation, descending — and that
/// order is meaningful: the container somebody just started belongs at the top. So this does not
/// replace it, it completes it.
///
/// The tiebreak is the point. Two rows created in the same second have no defined order between
/// them, which is the same churn [`sorted_by_name`] exists for, arriving once in a while instead of
/// every time — and once in a while is worse, because it looks like a glitch rather than a bug.
/// Both were measured stable on this machine across repeated reads; neither is *promised* to be,
/// and the cost of not relying on the promise is one comparison.
fn sorted_by_recency<T, K: FnMut(&T) -> (i64, String)>(mut rows: Vec<T>, mut key: K) -> Vec<T> {
    rows.sort_by(|a, b| {
        let (a_when, a_id) = key(a);
        let (b_when, b_id) = key(b);
        b_when.cmp(&a_when).then_with(|| a_id.cmp(&b_id))
    });
    rows
}

/// How much disk each volume is using, by name. (M57)
///
/// # Why this is its own call and never part of a board read
///
/// Because `GET /volumes` does not carry a size — measured, not assumed — and the only endpoint
/// that does is `/system/df`, which **walks the filesystem**. On this machine it costs **8.4
/// seconds cold** and about 1.2 warm, the daemon caching the answer in between. `docker volume ls`
/// shows no size for exactly the same reason, and `docker system df` is the separate, slower
/// command you run when you want one.
///
/// So this is asked only when a volume's detail is open, and never from `snapshot` — putting it
/// there would make every refresh, and every one of the daemon's own events, an eight-second
/// filesystem walk.
///
/// # Why the `type=volume` filter is not used, though the endpoint has one
///
/// `bollard`'s builder takes it as a `Vec<String>` and then cannot encode it — the call fails with
/// *Unable to URLEncode: unsupported value*, because `serde_urlencoded` has no repeated-key form.
/// Found by trying it. So the whole walk is asked for, which is what `docker system df` does
/// anyway, and the daemon's cache is what makes the second call cheap. If a narrower call is ever
/// wanted it needs a raw query, not this builder.
///
/// # The shape is read out of untyped JSON on purpose
///
/// `bollard` models the response as `VolumesDiskUsage { items: Option<Vec<serde_json::Value>> }` —
/// the items are not typed by the crate. Rather than reach past that, the two fields cide needs
/// are deserialised here into a private struct, which is the same discipline `cide_spec::model`
/// states: the *foreign* API's shape is `Deserialize`-only and never reaches the wire.
pub(crate) async fn volume_usage(
    docker: &bollard::Docker,
) -> Result<std::collections::HashMap<String, i64>, DockerError> {
    /// Only the two fields that are wanted, named as the daemon spells them.
    #[derive(serde::Deserialize)]
    struct Item {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "UsageData")]
        usage: Option<Usage>,
    }

    #[derive(serde::Deserialize)]
    struct Usage {
        #[serde(rename = "Size")]
        size: i64,
    }

    let answer = docker
        .df(None::<bollard::query_parameters::DataUsageOptions>)
        .await
        .map_err(|error| unreadable("the disk usage", &error))?;

    let mut sizes = std::collections::HashMap::new();
    for value in answer
        .volume_usage
        .and_then(|usage| usage.items)
        .unwrap_or_default()
    {
        // A row cide cannot read is skipped rather than failing the call: one unfamiliar volume
        // must not cost every other volume its size.
        if let Ok(item) = serde_json::from_value::<Item>(value)
            && let Some(usage) = item.usage
            // `-1` is the daemon saying it did not measure, which is not "empty" — the same
            // distinction `ImageRow::containers` carries, and the reason this is a map with
            // absences rather than a map full of zeroes.
            && usage.size >= 0
        {
            sizes.insert(item.name, usage.size);
        }
    }
    Ok(sizes)
}

/// Remove an image, a volume or a network — and **refuse if anything is using it**. (M56)
///
/// # Nothing here forces, and that is the feature
///
/// The exact opposite of [`act`]'s `Remove` one function below, which forces on purpose. Docker
/// already refuses each of these by default, with a sentence that names *what* is using it:
///
/// * an image — `image is being used by stopped container a1b2c3`;
/// * a volume — `volume is in use - [a1b2c3…]`;
/// * a network — `has active endpoints`.
///
/// Those sentences are worth more than the removal would be. A container is cattle and can be
/// recreated from its image in a second; an image is a multi-gigabyte download, a volume may hold
/// the only copy of somebody's database, and a network being torn out from under a running
/// container breaks it in a way that is not obvious afterwards. There is no undo for any of the
/// three, so the daemon's refusal is passed through rather than argued with — `docker rmi -f` is
/// a deliberate extra gesture at a terminal, and it stays one.
///
/// A predefined network (`bridge`, `host`, `none`) refuses for its own reason and says so; cide
/// does not need a list of them.
pub(crate) async fn remove(
    docker: &bollard::Docker,
    target: &cide_ipc::docker::Removable,
) -> Result<(), DockerError> {
    use bollard::query_parameters as q;
    use cide_ipc::docker::Removable;
    let refused = |error: &bollard::errors::Error| DockerError::Refused(daemon_words(error));

    match target {
        Removable::Image { id } => docker
            .remove_image(
                id,
                // `force(false)` stated rather than left to the default: it is the whole of this
                // function's contract, and a default is a thing a later reader can change without
                // noticing what they changed.
                Some(q::RemoveImageOptionsBuilder::default().force(false).build()),
                None,
            )
            .await
            // The daemon answers with what it deleted — untagged layers included. Dropped: the
            // board is re-read and emitted by the caller, which is the honest answer to "what
            // happened", and a list of layer digests is not something to put on screen.
            .map(|_| ())
            .map_err(|e| refused(&e)),
        Removable::Volume { name } => docker
            .remove_volume(
                name,
                Some(
                    q::RemoveVolumeOptionsBuilder::default()
                        .force(false)
                        .build(),
                ),
            )
            .await
            .map_err(|e| refused(&e)),
        // No options at all — the Engine API has no force for a network, which is why this arm
        // reads shorter than the other two rather than differently.
        Removable::Network { id } => docker.remove_network(id).await.map_err(|e| refused(&e)),
    }
}

/// Do one thing to one container.
///
/// # `Remove` forces, and leaves volumes alone
///
/// Two choices worth stating, because both are irreversible in one direction. Forcing means the
/// button does what it says on a *running* container instead of refusing with a daemon error the
/// user then has to translate — the frontend confirms first, so the intent is already explicit.
/// Not removing volumes is the opposite bias: a container is cattle and its volume may be the
/// only copy of somebody's database, and there is no undo. `docker rm -v` is a deliberate extra
/// gesture everywhere else too.
pub(crate) async fn act(
    docker: &bollard::Docker,
    id: &str,
    action: ContainerAction,
) -> Result<(), DockerError> {
    use bollard::query_parameters as q;
    let refused = |error: &bollard::errors::Error| DockerError::Refused(daemon_words(error));

    match action {
        ContainerAction::Start => docker
            .start_container(id, None::<q::StartContainerOptions>)
            .await
            .map_err(|e| refused(&e)),
        ContainerAction::Stop => docker
            .stop_container(id, None::<q::StopContainerOptions>)
            .await
            .map_err(|e| refused(&e)),
        ContainerAction::Restart => docker
            .restart_container(id, None::<q::RestartContainerOptions>)
            .await
            .map_err(|e| refused(&e)),
        ContainerAction::Pause => docker.pause_container(id).await.map_err(|e| refused(&e)),
        ContainerAction::Unpause => docker.unpause_container(id).await.map_err(|e| refused(&e)),
        ContainerAction::Kill => docker
            .kill_container(id, None::<q::KillContainerOptions>)
            .await
            .map_err(|e| refused(&e)),
        ContainerAction::Remove => docker
            .remove_container(
                id,
                Some(
                    q::RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(false)
                        .build(),
                ),
            )
            .await
            .map_err(|e| refused(&e)),
    }
}

/// Every context as a row. `pub(crate)` because a failed connection still shows the switcher —
/// see `cide_ipc::docker::DockerBoard::Unusable`.
pub(crate) fn context_rows(contexts: &[Context]) -> Vec<ContextRow> {
    contexts.iter().map(context_row).collect()
}

fn context_row(context: &Context) -> ContextRow {
    ContextRow {
        name: context.name.clone(),
        description: context.description.clone(),
        endpoint: context.endpoint.as_url(),
        current: context.current,
    }
}

/// One `ContainerSummary` as a row, or `None` for a summary with no id.
///
/// A container with no id cannot be started, stopped, inspected or logged — every gesture the
/// row exists to offer is addressed by it. Drawing an inert row would be worse than drawing none.
fn container_row(summary: &bollard::models::ContainerSummary) -> Option<ContainerRow> {
    let id = summary.id.clone()?;
    let labels = summary.labels.clone().unwrap_or_default();
    Some(ContainerRow {
        // Docker prefixes every name with `/` for historic reasons ("links"), and a container
        // may carry several. The first is the one every CLI shows.
        name: summary
            .names
            .as_ref()
            .and_then(|names| names.first())
            .map(|name| name.trim_start_matches('/').to_string())
            .unwrap_or_else(|| id.chars().take(12).collect()),
        image: summary.image.clone().unwrap_or_default(),
        state: summary
            .state
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        status: summary.status.clone().unwrap_or_default(),
        health: summary
            .health
            .as_ref()
            .and_then(|h| h.status.as_ref())
            .map(ToString::to_string)
            // **Two** spellings of "no healthcheck", and the second was found by running this
            // against a real daemon rather than by reading the enum: `EMPTY` renders as `""` and
            // is what a summary with no `Health` object at all produces, while a live Docker
            // Engine sends `"none"` for every container whose image declares no healthcheck —
            // which is most of them. Filtering only the empty string put a `[none]` badge on
            // every ordinary container on the machine M41 was written on.
            .filter(|status| !status.is_empty() && status != "none"),
        created: summary.created.unwrap_or_default(),
        ports: summary
            .ports
            .as_ref()
            .map(|ports| dedupe_ports(ports.iter().map(port_row).collect()))
            .unwrap_or_default(),
        compose: compose_membership(&labels),
        id,
    })
}

/// Collapse the rows a dual-stack publish produces into one.
///
/// # Why this is not over-cleaning
///
/// Publishing a port on a machine with IPv6 makes the daemon report it **twice** — once bound to
/// `0.0.0.0` and once to `::` — and [`port_row`] drops both of those addresses because "every
/// interface" is the default and says nothing. What is left is two rows that are equal in every
/// field, which is not two ports and must not draw as `5433->5432/tcp 5433->5432/tcp`. Found
/// against a real daemon; every container on that machine with a published port had it.
///
/// Rows that differ in *any* field are kept, deliberately: a port published on `127.0.0.1` and
/// again on a LAN address is two genuinely different facts, and it is the one somebody is
/// checking when they ask whether a database is exposed.
fn dedupe_ports(mut ports: Vec<PortRow>) -> Vec<PortRow> {
    let mut seen = Vec::new();
    ports.retain(|port| {
        if seen.contains(port) {
            return false;
        }
        seen.push(port.clone());
        true
    });
    ports
}

fn port_row(port: &bollard::models::PortSummary) -> PortRow {
    PortRow {
        private: port.private_port,
        public: port.public_port,
        protocol: port
            .typ
            .as_ref()
            .map(ToString::to_string)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "tcp".to_string()),
        host_ip: port
            .ip
            .clone()
            // `0.0.0.0` is "every interface", which is the default and says nothing. Carrying it
            // would put it in front of every published port in the panel.
            .filter(|ip| !ip.is_empty() && ip != "0.0.0.0" && ip != "::"),
    }
}

/// The compose labels, or `None` for a container nobody composed.
///
/// Both `project` and `service` are required. A container carrying one and not the other is not
/// a stack member cide can act on — `docker compose` addresses a service within a project — and
/// inventing the missing half would put a row under a stack that cannot control it.
fn compose_membership(labels: &HashMap<String, String>) -> Option<ComposeMembership> {
    let project = labels.get(LABEL_PROJECT)?.clone();
    let service = labels.get(LABEL_SERVICE)?.clone();
    if project.is_empty() || service.is_empty() {
        return None;
    }
    Some(ComposeMembership {
        project,
        service,
        working_dir: labels.get(LABEL_WORKING_DIR).cloned(),
        config_files: labels
            .get(LABEL_CONFIG_FILES)
            .map(|files| {
                files
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn image_row(summary: &bollard::models::ImageSummary) -> ImageRow {
    ImageRow {
        id: summary.id.clone(),
        // `<none>:<none>` is how Docker spells "untagged" in a list, and it is a *string* in the
        // tag array rather than an absent entry. Carried through, an image would show a tag it
        // does not have and a prune would look like it had missed something.
        tags: summary
            .repo_tags
            .iter()
            .filter(|tag| !tag.is_empty() && *tag != "<none>:<none>")
            .cloned()
            .collect(),
        created: summary.created,
        size: summary.size,
        containers: summary.containers,
        kind: "image".to_string(),
    }
}

fn volume_row(volume: &bollard::models::Volume) -> VolumeRow {
    VolumeRow {
        name: volume.name.clone(),
        driver: volume.driver.clone(),
        mountpoint: volume.mountpoint.clone(),
        project: volume
            .labels
            .get(LABEL_PROJECT)
            .cloned()
            .filter(|p| !p.is_empty()),
        // `UsageData` is only populated when the caller asked for it, and `GET /volumes` never
        // does — so this is `None` here rather than `Some(0)`, which is the difference between
        // "cide did not ask" and "nothing would break if you removed this".
        in_use_by: volume
            .usage_data
            .as_ref()
            .map(|usage| usage.ref_count)
            .filter(|count| *count >= 0),
    }
}

/// One `Network` as a row, or `None` for one with no id.
///
/// `container_row`'s rule: every gesture a row could offer is addressed by the id, so a network
/// without one has nothing a row could do.
fn network_row(network: &bollard::models::Network) -> Option<NetworkRow> {
    let id = network.id.clone()?;
    Some(NetworkRow {
        name: network
            .name
            .clone()
            .unwrap_or_else(|| id.chars().take(12).collect()),
        driver: network.driver.clone().unwrap_or_default(),
        scope: network.scope.clone().unwrap_or_default(),
        project: network
            .labels
            .as_ref()
            .and_then(|labels| labels.get(LABEL_PROJECT))
            .cloned()
            .filter(|p| !p.is_empty()),
        // Empty for a driver that has no subnets — `host` and `none` both do — and that is a
        // fact rather than a gap, so it renders as nothing rather than as "unknown".
        subnets: network
            .ipam
            .as_ref()
            .and_then(|ipam| ipam.config.as_ref())
            .map(|configs| {
                configs
                    .iter()
                    .filter_map(|config| config.subnet.clone())
                    .filter(|subnet| !subnet.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        id,
    })
}

/// The daemon did not answer. Names the endpoint, because "connection refused" without one is a
/// sentence the user cannot act on — the whole question is *which* daemon refused.
fn unreachable(endpoint: &Endpoint, error: &bollard::errors::Error) -> DockerError {
    DockerError::Unreachable(format!(
        "cide could not reach the Docker daemon at `{}`: {}",
        endpoint.as_url(),
        daemon_words(error)
    ))
}

fn unreadable(what: &str, error: &bollard::errors::Error) -> DockerError {
    DockerError::Unreadable {
        what: what.to_string(),
        detail: daemon_words(error),
    }
}

/// The daemon's own sentence, where there is one.
///
/// A `DockerResponseServerError` carries the message the daemon wrote, which is nearly always
/// better than anything cide could compose — "You cannot remove a running container" says the
/// remedy. Everything else falls back to the transport error, which for a socket is the one
/// that matters: `No such file or directory` means the daemon is not up, `Permission denied`
/// means the user is not in the `docker` group, and those are different remedies.
pub(crate) fn daemon_words(error: &bollard::errors::Error) -> String {
    match error {
        bollard::errors::Error::DockerResponseServerError { message, .. }
            if !message.trim().is_empty() =>
        {
            message.trim().to_string()
        }
        other => other.to_string(),
    }
}

/// The raw `inspect` document, pretty-printed.
///
/// # Why the document is re-serialised rather than passed through
///
/// The daemon sends one line. bollard hands back a *typed* struct, which would lose every field
/// this build does not model — so the request is made through the typed call and the answer is
/// re-serialised from it, which keeps every field bollard models and, crucially, keeps the
/// **order** stable across reads. A user comparing two containers side by side is comparing
/// documents, and two orderings would make every line differ.
pub(crate) fn inspect(
    docker: &bollard::Docker,
    runtime: &tokio::runtime::Runtime,
    target: &cide_ipc::docker::InspectTarget,
) -> Result<String, DockerError> {
    use cide_ipc::docker::InspectTarget;
    runtime.block_on(async {
        let value: serde_json::Value = match target {
            InspectTarget::Container { id } => {
                let it = docker
                    .inspect_container(
                        id,
                        None::<bollard::query_parameters::InspectContainerOptions>,
                    )
                    .await
                    .map_err(|e| unreadable("this container", &e))?;
                serde_json::to_value(it)
            }
            InspectTarget::Image { id } => {
                let it = docker
                    .inspect_image(id)
                    .await
                    .map_err(|e| unreadable("this image", &e))?;
                serde_json::to_value(it)
            }
            InspectTarget::Volume { name } => {
                let it = docker
                    .inspect_volume(name)
                    .await
                    .map_err(|e| unreadable("this volume", &e))?;
                serde_json::to_value(it)
            }
            InspectTarget::Network { id } => {
                let it = docker
                    .inspect_network(id, None::<bollard::query_parameters::InspectNetworkOptions>)
                    .await
                    .map_err(|e| unreadable("this network", &e))?;
                serde_json::to_value(it)
            }
        }
        .map_err(|error| DockerError::Unreadable {
            what: "an inspect document".to_string(),
            detail: error.to_string(),
        })?;

        serde_json::to_string_pretty(&value).map_err(|error| DockerError::Unreadable {
            what: "an inspect document".to_string(),
            detail: error.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unordered_list_is_sorted_by_name_case_insensitively() {
        // The bug this exists for: `GET /networks` and `GET /volumes` answer in a *different order
        // on every call* — six consecutive reads against a live daemon gave six orders — and the
        // panel refetches the whole board on every daemon event, so those two sections reshuffled
        // several times a minute under the user's pointer.
        let rows = vec!["Redis_default", "bridge", "minikube", "Alpha", "none"];
        let sorted = sorted_by_name(rows, |row| (*row).to_string());
        assert_eq!(
            sorted,
            vec!["Alpha", "bridge", "minikube", "none", "Redis_default"],
            "case-insensitive: sorting by byte value puts every capital ahead of every lowercase, \
             which reads as no order at all",
        );

        // Idempotent, which is the property that actually matters here — two reads of the same
        // set must draw the same list whatever order they arrived in.
        let shuffled = vec!["none", "Alpha", "Redis_default", "minikube", "bridge"];
        assert_eq!(
            sorted_by_name(shuffled, |row| (*row).to_string()),
            sorted,
            "the same set in a different order sorts to the same list",
        );
    }

    #[test]
    fn recency_is_kept_and_a_tie_is_broken_by_id() {
        // `containers/json` and `images/json` *are* ordered — newest first — and that order is
        // meaningful, so it is completed rather than replaced.
        let rows = vec![(10i64, "b"), (30, "a"), (20, "c")];
        assert_eq!(
            sorted_by_recency(rows, |row| (row.0, row.1.to_string())),
            vec![(30, "a"), (20, "c"), (10, "b")],
            "newest first",
        );

        // The tiebreak. Two rows created in the same second have no defined order between them,
        // which is the same churn as above arriving once in a while instead of every time — and
        // once in a while is worse, because it reads as a glitch rather than a bug.
        let tied = vec![(5i64, "z"), (5, "a"), (5, "m")];
        assert_eq!(
            sorted_by_recency(tied, |row| (row.0, row.1.to_string())),
            vec![(5, "a"), (5, "m"), (5, "z")],
        );
    }

    fn labels(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn a_summary_with_no_id_is_skipped_rather_than_drawn_inert() {
        // Every gesture the row offers is addressed by the id.
        let mut summary = bollard::models::ContainerSummary::default();
        assert!(container_row(&summary).is_none());
        summary.id = Some("a".repeat(64));
        assert!(container_row(&summary).is_some());
    }

    #[test]
    fn a_name_keeps_no_leading_slash_and_an_unnamed_container_shows_its_short_id() {
        let mut summary = bollard::models::ContainerSummary {
            id: Some("0123456789abcdef".to_string() + &"0".repeat(48)),
            names: Some(vec!["/web".to_string(), "/other".to_string()]),
            ..Default::default()
        };
        assert_eq!(container_row(&summary).unwrap().name, "web");

        summary.names = None;
        assert_eq!(
            container_row(&summary).unwrap().name,
            "0123456789ab",
            "an empty cell would read as a container with no name rather than none reported"
        );
    }

    #[test]
    fn no_healthcheck_is_none_and_never_an_empty_badge() {
        // The enum's `EMPTY` variant renders as "", and a panel that drew a health badge for
        // every non-`None` value would put an empty red pill on most containers in the world.
        let mut summary = bollard::models::ContainerSummary {
            id: Some("a".repeat(64)),
            ..Default::default()
        };
        summary.health = Some(bollard::models::ContainerSummaryHealth {
            status: Some(bollard::models::ContainerSummaryHealthStatusEnum::EMPTY),
            ..Default::default()
        });
        assert_eq!(container_row(&summary).unwrap().health, None);

        // The spelling a *live* daemon actually sends for an image with no healthcheck. The
        // synthetic case above passed while this one shipped a `[none]` badge on every ordinary
        // container, which is why this assertion exists separately.
        summary.health = Some(bollard::models::ContainerSummaryHealth {
            status: Some(bollard::models::ContainerSummaryHealthStatusEnum::NONE),
            ..Default::default()
        });
        assert_eq!(container_row(&summary).unwrap().health, None);

        summary.health = Some(bollard::models::ContainerSummaryHealth {
            status: Some(bollard::models::ContainerSummaryHealthStatusEnum::UNHEALTHY),
            ..Default::default()
        });
        assert_eq!(
            container_row(&summary).unwrap().health.as_deref(),
            Some("unhealthy")
        );
    }

    #[test]
    fn a_compose_membership_needs_both_halves() {
        // `docker compose` addresses a service within a project. Half a membership would put a
        // row under a stack whose buttons cannot control it.
        assert!(compose_membership(&labels(&[])).is_none());
        assert!(compose_membership(&labels(&[(LABEL_PROJECT, "shop")])).is_none());
        assert!(compose_membership(&labels(&[(LABEL_SERVICE, "web")])).is_none());
        assert!(
            compose_membership(&labels(&[(LABEL_PROJECT, ""), (LABEL_SERVICE, "web")])).is_none(),
            "an empty label is not a project"
        );

        let member = compose_membership(&labels(&[
            (LABEL_PROJECT, "shop"),
            (LABEL_SERVICE, "web"),
            (LABEL_WORKING_DIR, "/home/u/shop"),
            (
                LABEL_CONFIG_FILES,
                "/home/u/shop/compose.yaml,/home/u/shop/compose.dev.yaml",
            ),
        ]))
        .expect("both halves are present");
        assert_eq!(member.project, "shop");
        assert_eq!(member.working_dir.as_deref(), Some("/home/u/shop"));
        assert_eq!(member.config_files.len(), 2, "the label is comma-separated");
    }

    #[test]
    fn every_interface_is_not_a_host_address() {
        // `0.0.0.0` is the default and says nothing; carried through it would sit in front of
        // every published port in the panel.
        let port = |ip: Option<&str>| bollard::models::PortSummary {
            ip: ip.map(str::to_string),
            private_port: 80,
            public_port: Some(8080),
            typ: Some(bollard::models::PortSummaryTypeEnum::TCP),
        };
        assert_eq!(port_row(&port(Some("0.0.0.0"))).host_ip, None);
        assert_eq!(port_row(&port(Some("::"))).host_ip, None);
        assert_eq!(port_row(&port(None)).host_ip, None);
        assert_eq!(
            port_row(&port(Some("127.0.0.1"))).host_ip.as_deref(),
            Some("127.0.0.1"),
            "a loopback-only binding is exactly the fact somebody is looking for"
        );
    }

    #[test]
    fn an_untagged_image_has_no_tags_rather_than_a_none_none_one() {
        // `<none>:<none>` is a *string* in the tag array, not an absent entry. Carried through,
        // a dangling image shows a tag it does not have and a prune looks like it missed one.
        let summary = bollard::models::ImageSummary {
            id: "sha256:abc".to_string(),
            repo_tags: vec!["<none>:<none>".to_string()],
            containers: -1,
            ..Default::default()
        };
        let row = image_row(&summary);
        assert!(row.tags.is_empty());
        assert_eq!(
            row.containers, -1,
            "Docker's sentinel is kept: 0 means safe to remove, -1 means cide does not know"
        );
    }

    #[test]
    fn a_dual_stack_publish_is_one_port_and_not_two() {
        // A published port on an IPv6-capable host is reported twice, bound to `0.0.0.0` and to
        // `::`. Both addresses mean "every interface" and are dropped, which leaves two rows
        // equal in every field — one port, drawn twice.
        let published = |ip: &str| bollard::models::PortSummary {
            ip: Some(ip.to_string()),
            private_port: 5432,
            public_port: Some(5433),
            typ: Some(bollard::models::PortSummaryTypeEnum::TCP),
        };
        let rows = dedupe_ports(vec![
            port_row(&published("0.0.0.0")),
            port_row(&published("::")),
        ]);
        assert_eq!(rows.len(), 1);

        // And the half that must not be cleaned away: a loopback-only binding beside a public
        // one is two different facts, and it is the one somebody checks before asking whether a
        // database is exposed.
        let rows = dedupe_ports(vec![
            port_row(&published("127.0.0.1")),
            port_row(&published("10.0.0.4")),
        ]);
        assert_eq!(rows.len(), 2);
    }
}
