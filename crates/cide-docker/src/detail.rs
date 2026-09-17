//! What the detail pane shows, and the one edit Docker allows. (M47)
//!
//! # Structured here, raw in the tab
//!
//! `inspect` answers a two-hundred-field document and `TabKind::Docker` shows all of it. This
//! module produces the handful of fields somebody actually looks at — ports, mounts, env, labels,
//! networks — because a pane that made the user read JSON to find a port number would be a worse
//! answer than the `docker` CLI they already have. Both surfaces exist and neither replaces the
//! other.
//!
//! # Editing a container means replacing it
//!
//! There is no endpoint that changes a running container's ports. None that changes its
//! environment, its mounts, or its command either — the container is immutable once created, and
//! `POST /containers/{id}/update` covers only resource limits and the restart policy. Every tool
//! that appears to edit one removes it and creates another from the same image under the same
//! name, and [`recreate`] does exactly that.
//!
//! Two consequences that cannot be designed away, both stated here because neither is visible
//! from the call site:
//!
//! * **The id changes.** An exec pane or a log follow holding the old one is attached to a
//!   container that no longer exists; they end, and the panel's rows re-point on the next read.
//! * **Anything not carried over is lost.** So the new body is built from the old container's own
//!   `inspect` rather than from scratch, and the edit supplies only what it changes. A field this
//!   module forgets to copy is a field that silently reverts to the image's default — which is
//!   why the copy is one struct literal with every field named, rather than a `..Default::default()`
//!   that would hide the omission.

use cide_ipc::docker::{
    ComposeMembership, ContainerDetail, ContainerEdit, DockerDetail, ImageDetail, MountRow,
    NetworkDetail, Pair, PortRow, VolumeDetail,
};

use crate::{Docker, DockerError};

/// The compose labels, as `api` reads them.
const LABEL_PROJECT: &str = "com.docker.compose.project";
const LABEL_SERVICE: &str = "com.docker.compose.service";
const LABEL_WORKING_DIR: &str = "com.docker.compose.project.working_dir";

/// Sorted key/value pairs from a label or environment map.
///
/// Sorted because the daemon's order is a hash map's and differs between reads: an unsorted list
/// would make the pane reshuffle every refresh, which is the same argument `groupByCompose` makes
/// one layer up.
fn pairs(map: &std::collections::HashMap<String, String>) -> Vec<Pair> {
    let mut out: Vec<Pair> = map
        .iter()
        .map(|(name, value)| Pair {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `KEY=value` strings as pairs, which is how the API spells an environment.
///
/// A variable whose value contains `=` splits on the **first** one only — `JAVA_OPTS=-Da=b` is one
/// variable, not a malformed line.
fn env_pairs(env: &[String]) -> Vec<Pair> {
    let mut out: Vec<Pair> = env
        .iter()
        .map(|line| match line.split_once('=') {
            Some((name, value)) => Pair {
                name: name.to_string(),
                value: value.to_string(),
            },
            // A bare name with no `=` is legal and means "inherit"; it is shown as itself rather
            // than as a variable set to the empty string, which is a different thing.
            None => Pair {
                name: line.clone(),
                value: String::new(),
            },
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The entrypoint and command as one line, the way `docker ps` shows it.
fn command_line(entrypoint: Option<&Vec<String>>, cmd: Option<&Vec<String>>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(entrypoint) = entrypoint {
        parts.extend(entrypoint.iter().cloned());
    }
    if let Some(cmd) = cmd {
        parts.extend(cmd.iter().cloned());
    }
    parts.join(" ")
}

impl Docker {
    /// What the detail pane shows for one thing.
    pub fn detail(&self, target: &cide_ipc::docker::InspectTarget) -> DockerDetail {
        use cide_ipc::docker::InspectTarget;
        let answer = match target {
            InspectTarget::Container { id } => {
                self.container_detail(id).map(DockerDetail::Container)
            }
            InspectTarget::Image { id } => self.image_detail(id).map(DockerDetail::Image),
            InspectTarget::Volume { name } => self.volume_detail(name).map(DockerDetail::Volume),
            InspectTarget::Network { id } => self.network_detail(id).map(DockerDetail::Network),
        };
        match answer {
            Ok(detail) => detail,
            // A thing removed between the row being drawn and the row being clicked is an
            // ordinary outcome here, not an error — see `DockerDetail::Missing`.
            Err(error) => DockerDetail::Missing {
                reason: error.to_string(),
            },
        }
    }

    fn container_detail(&self, id: &str) -> Result<Box<ContainerDetail>, DockerError> {
        let (client, runtime) = self.parts();
        let it = runtime
            .block_on(client.inspect_container(
                id,
                None::<bollard::query_parameters::InspectContainerOptions>,
            ))
            .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

        let config = it.config.clone().unwrap_or_default();
        let host = it.host_config.clone().unwrap_or_default();
        let labels = config.labels.clone().unwrap_or_default();
        let state = it.state.clone().unwrap_or_default();

        Ok(Box::new(ContainerDetail {
            name: it
                .name
                .clone()
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            image: config.image.clone().unwrap_or_default(),
            state: state
                .status
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            status: state
                .status
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            created: 0,
            command: command_line(config.entrypoint.as_ref(), config.cmd.as_ref()),
            ports: published_ports(&host),
            mounts: it
                .mounts
                .unwrap_or_default()
                .iter()
                .map(mount_row)
                .collect(),
            env: env_pairs(&config.env.clone().unwrap_or_default()),
            labels: pairs(&labels),
            networks: it
                .network_settings
                .and_then(|settings| settings.networks)
                .map(|nets| {
                    let mut names: Vec<String> = nets.keys().cloned().collect();
                    names.sort();
                    names
                })
                .unwrap_or_default(),
            restart_policy: host
                .restart_policy
                .and_then(|policy| policy.name)
                .map(|name| name.to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "no".to_string()),
            compose: membership(&labels),
            id: it.id.unwrap_or_else(|| id.to_string()),
        }))
    }

    fn image_detail(&self, id: &str) -> Result<Box<ImageDetail>, DockerError> {
        let (client, runtime) = self.parts();
        let it = runtime
            .block_on(client.inspect_image(id))
            .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;
        let config = it.config.clone().unwrap_or_default();
        Ok(Box::new(ImageDetail {
            id: it.id.unwrap_or_else(|| id.to_string()),
            tags: it
                .repo_tags
                .unwrap_or_default()
                .into_iter()
                .filter(|tag| tag != "<none>:<none>")
                .collect(),
            created: 0,
            size: it.size.unwrap_or_default(),
            architecture: it.architecture.unwrap_or_default(),
            os: it.os.unwrap_or_default(),
            command: command_line(config.entrypoint.as_ref(), config.cmd.as_ref()),
            env: env_pairs(&config.env.clone().unwrap_or_default()),
            labels: pairs(&config.labels.clone().unwrap_or_default()),
            exposed: {
                let mut ports = config.exposed_ports.unwrap_or_default();
                ports.sort();
                ports
            },
        }))
    }

    fn volume_detail(&self, name: &str) -> Result<Box<VolumeDetail>, DockerError> {
        let (client, runtime) = self.parts();
        let it = runtime
            .block_on(client.inspect_volume(name))
            .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;
        Ok(Box::new(VolumeDetail {
            name: it.name,
            driver: it.driver,
            mountpoint: it.mountpoint,
            scope: it.scope.map(|s| s.to_string()).unwrap_or_default(),
            labels: pairs(&it.labels),
            options: pairs(&it.options),
        }))
    }

    fn network_detail(&self, id: &str) -> Result<Box<NetworkDetail>, DockerError> {
        let (client, runtime) = self.parts();
        let it = runtime
            .block_on(
                client
                    .inspect_network(id, None::<bollard::query_parameters::InspectNetworkOptions>),
            )
            .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;
        Ok(Box::new(NetworkDetail {
            name: it.name.clone().unwrap_or_default(),
            driver: it.driver.clone().unwrap_or_default(),
            scope: it.scope.clone().unwrap_or_default(),
            internal: it.internal.unwrap_or(false),
            subnets: it
                .ipam
                .and_then(|ipam| ipam.config)
                .map(|configs| configs.iter().filter_map(|c| c.subnet.clone()).collect())
                .unwrap_or_default(),
            labels: pairs(&it.labels.clone().unwrap_or_default()),
            attached: {
                let mut names: Vec<String> = it
                    .containers
                    .unwrap_or_default()
                    .into_values()
                    .filter_map(|c| c.name)
                    .collect();
                names.sort();
                names
            },
            id: it.id.unwrap_or_else(|| id.to_string()),
        }))
    }

    /// Replace a container with one that differs only in what the edit names. (M47)
    ///
    /// Remove, create, start — in that order, because the name has to be free before the new one
    /// can take it, and a container's name is the only thing about it a user recognises.
    ///
    /// Returns the **new** id. The old one is gone by then, and a caller holding it — an exec
    /// pane, a log follow — is holding a container that no longer exists.
    pub fn recreate(&self, id: &str, edit: &ContainerEdit) -> Result<String, DockerError> {
        let (client, runtime) = self.parts();
        runtime.block_on(async {
            let it = client
                .inspect_container(
                    id,
                    None::<bollard::query_parameters::InspectContainerOptions>,
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            let name = it
                .name
                .clone()
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string();
            let was_running = it.state.as_ref().and_then(|s| s.running).unwrap_or(false);
            let config = it.config.clone().ok_or_else(|| {
                DockerError::Refused(
                    "this container reports no configuration, so cide cannot rebuild it".into(),
                )
            })?;
            let mut host = it.host_config.clone().unwrap_or_default();

            // The one edit. Both halves move together: `exposed_ports` is what the *image*
            // offers and `port_bindings` is what the host publishes, and a binding whose port is
            // not exposed is silently ignored by the daemon.
            host.port_bindings = Some(bindings_for(&edit.ports));
            let mut exposed = config.exposed_ports.clone().unwrap_or_default();
            for port in &edit.ports {
                let spec = format!("{}/{}", port.private, port.protocol);
                if !exposed.contains(&spec) {
                    exposed.push(spec);
                }
            }
            exposed.sort();

            // The networks it was attached to, so a recreated container comes back on the same
            // ones. Without this a compose service returns on the default bridge and stops
            // resolving its siblings by name — which looks like the *edit* broke networking.
            let networking = it.network_settings.as_ref().and_then(|settings| {
                settings.networks.as_ref().map(|nets| {
                    bollard::models::NetworkingConfig {
                        endpoints_config: Some(
                            nets.iter()
                                .map(|(name, endpoint)| {
                                    (
                                        name.clone(),
                                        bollard::models::EndpointSettings {
                                            // Aliases are how compose services find each other.
                                            aliases: endpoint.aliases.clone(),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .collect(),
                        ),
                    }
                })
            });

            // **Every field named.** `ContainerConfig` and `ContainerCreateBody` have the same
            // field names and are different types, so there is no `..config` to fall back on —
            // and that is the safer shape anyway: a field left out here is a setting that
            // silently reverts to the image's default, and a literal makes the omission visible
            // in review rather than invisible at runtime.
            let body = bollard::models::ContainerCreateBody {
                hostname: config.hostname.clone(),
                domainname: config.domainname.clone(),
                user: config.user.clone(),
                attach_stdin: config.attach_stdin,
                attach_stdout: config.attach_stdout,
                attach_stderr: config.attach_stderr,
                exposed_ports: Some(exposed),
                tty: config.tty,
                open_stdin: config.open_stdin,
                stdin_once: config.stdin_once,
                env: config.env.clone(),
                cmd: config.cmd.clone(),
                healthcheck: config.healthcheck.clone(),
                args_escaped: config.args_escaped,
                image: config.image.clone(),
                volumes: config.volumes.clone(),
                working_dir: config.working_dir.clone(),
                entrypoint: config.entrypoint.clone(),
                network_disabled: config.network_disabled,
                on_build: config.on_build.clone(),
                labels: config.labels.clone(),
                stop_signal: config.stop_signal.clone(),
                stop_timeout: config.stop_timeout,
                shell: config.shell.clone(),
                host_config: Some(host),
                networking_config: networking,
            };

            client
                .remove_container(
                    id,
                    Some(
                        bollard::query_parameters::RemoveContainerOptionsBuilder::default()
                            .force(true)
                            // Never `v`: a recreate must not take the data with it. That is the
                            // difference between editing a port and losing a database.
                            .v(false)
                            .build(),
                    ),
                )
                .await
                .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;

            let created = client
                .create_container(
                    Some(
                        bollard::query_parameters::CreateContainerOptionsBuilder::default()
                            .name(&name)
                            .build(),
                    ),
                    body,
                )
                .await
                .map_err(|e| {
                    // The old container is already gone. Say so, because the user is looking at a
                    // panel that has just lost a row and needs to know the image is still there.
                    DockerError::Refused(format!(
                        "`{name}` was removed and the replacement could not be created: {}. The \
                         image is untouched — `docker run` can recreate it by hand.",
                        crate::api::daemon_words(&e)
                    ))
                })?;

            // Started only if it was running. A stopped container that a user edited should stay
            // stopped: starting it would be a second, unasked-for act on the same click.
            if was_running {
                client
                    .start_container(
                        &created.id,
                        None::<bollard::query_parameters::StartContainerOptions>,
                    )
                    .await
                    .map_err(|e| DockerError::Refused(crate::api::daemon_words(&e)))?;
            }
            Ok(created.id)
        })
    }
}

use std::collections::HashMap;

/// The published ports a host config describes.
fn published_ports(host: &bollard::models::HostConfig) -> Vec<PortRow> {
    let mut out: Vec<PortRow> = Vec::new();
    for (spec, bindings) in host.port_bindings.clone().unwrap_or_default() {
        let (private, protocol) = split_port(&spec);
        for binding in bindings.unwrap_or_default() {
            let row = PortRow {
                private,
                public: binding.host_port.and_then(|p| p.parse().ok()),
                protocol: protocol.clone(),
                host_ip: binding
                    .host_ip
                    .filter(|ip| !ip.is_empty() && ip != "0.0.0.0" && ip != "::"),
            };
            if !out.contains(&row) {
                out.push(row);
            }
        }
    }
    out.sort_by_key(|row| (row.private, row.public));
    out
}

/// `8080/tcp` as its two halves. A spec with no protocol is TCP, which is the API's own default.
fn split_port(spec: &str) -> (u16, String) {
    match spec.split_once('/') {
        Some((port, protocol)) => (port.parse().unwrap_or_default(), protocol.to_string()),
        None => (spec.parse().unwrap_or_default(), "tcp".to_string()),
    }
}

/// Rows as the `PortMap` the API takes.
fn bindings_for(ports: &[PortRow]) -> bollard::models::PortMap {
    let mut map: bollard::models::PortMap = HashMap::new();
    for port in ports {
        let spec = format!("{}/{}", port.private, port.protocol);
        let binding = bollard::models::PortBinding {
            host_ip: port.host_ip.clone(),
            // `None` rather than an empty string for an unpublished port: the daemon reads an
            // empty `HostPort` as "pick any free one", which is a different request.
            host_port: port.public.map(|p| p.to_string()),
        };
        map.entry(spec).or_insert_with(|| Some(Vec::new()));
        if let Some(Some(list)) = map.get_mut(&format!("{}/{}", port.private, port.protocol)) {
            list.push(binding);
        }
    }
    map
}

fn mount_row(mount: &bollard::models::MountPoint) -> MountRow {
    MountRow {
        kind: mount
            .typ
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        name: mount.name.clone().unwrap_or_default(),
        source: mount.source.clone().unwrap_or_default(),
        destination: mount.destination.clone().unwrap_or_default(),
        read_only: !mount.rw.unwrap_or(true),
    }
}

fn membership(labels: &HashMap<String, String>) -> Option<ComposeMembership> {
    let project = labels.get(LABEL_PROJECT)?.clone();
    let service = labels.get(LABEL_SERVICE)?.clone();
    if project.is_empty() || service.is_empty() {
        return None;
    }
    Some(ComposeMembership {
        project,
        service,
        working_dir: labels.get(LABEL_WORKING_DIR).cloned(),
        config_files: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_splits_on_the_first_equals_only() {
        // `JAVA_OPTS=-Da=b` is one variable, not a malformed line.
        let pairs = env_pairs(&["A=1".into(), "JAVA_OPTS=-Da=b".into(), "INHERIT".into()]);
        assert_eq!(
            pairs[0],
            Pair {
                name: "A".into(),
                value: "1".into()
            }
        );
        assert_eq!(
            pairs[1],
            Pair {
                name: "INHERIT".into(),
                value: String::new()
            },
            "a bare name means inherit and is shown as itself, sorted with the rest"
        );
        assert_eq!(
            pairs[2],
            Pair {
                name: "JAVA_OPTS".into(),
                value: "-Da=b".into()
            }
        );
    }

    #[test]
    fn a_port_spec_without_a_protocol_is_tcp() {
        assert_eq!(split_port("8080/udp"), (8080, "udp".to_string()));
        assert_eq!(split_port("80"), (80, "tcp".to_string()));
    }

    #[test]
    fn an_unpublished_port_binds_to_none_and_never_an_empty_string() {
        // The daemon reads an empty `HostPort` as "pick any free one", which is a different
        // request from "do not publish this".
        let map = bindings_for(&[PortRow {
            private: 80,
            public: None,
            protocol: "tcp".into(),
            host_ip: None,
        }]);
        let bindings = map
            .get("80/tcp")
            .expect("the spec is there")
            .clone()
            .unwrap();
        assert_eq!(bindings[0].host_port, None);
    }

    #[test]
    fn a_published_port_round_trips_through_the_binding_map() {
        let rows = vec![PortRow {
            private: 5432,
            public: Some(5433),
            protocol: "tcp".into(),
            host_ip: Some("127.0.0.1".into()),
        }];
        let map = bindings_for(&rows);
        let host = bollard::models::HostConfig {
            port_bindings: Some(map),
            ..Default::default()
        };
        assert_eq!(published_ports(&host), rows, "what goes in comes back out");
    }
}
