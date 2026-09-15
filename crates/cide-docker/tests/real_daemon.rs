//! Against a real Docker daemon. `#[ignore]`d by the workspace convention. (M41, M42)
//!
//! These need a daemon `cide_docker::connect` can find and at least one running container, and
//! they spend real time on real sockets — so they are run deliberately:
//!
//! ```sh
//! cargo test -p cide-docker -- --ignored --nocapture
//! ```
//!
//! # What these cover that the unit tests cannot
//!
//! Every unit test in this crate is a function of an injected value, which is what makes them run
//! on a machine with no Docker. That is also their limit: three real bugs in M41 — `health`
//! reporting `"none"`, a dual-stack publish arriving twice, and `<none>:<none>` as a literal tag —
//! were all cases where the fixture agreed with cide's code rather than with Docker. These are the
//! tests that disagree.

use std::sync::Arc;

use cide_docker::{Docker, connect};

/// A connection, or a skip. Never a failure: a machine with no daemon is not a broken build.
fn daemon() -> Option<Arc<Docker>> {
    match Docker::open(None) {
        Ok(docker) => Some(Arc::new(docker)),
        Err(error) => {
            eprintln!("skipped: {error}");
            None
        }
    }
}

#[test]
#[ignore = "needs a real Docker daemon"]
fn the_ladder_resolves_and_the_snapshot_is_coherent() {
    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");

    println!(
        "{} — {} api {}",
        snapshot.endpoint, snapshot.server, snapshot.api_version
    );
    assert!(
        !snapshot.api_version.is_empty(),
        "the version was negotiated"
    );

    for container in &snapshot.containers {
        assert_eq!(container.id.len(), 64, "the full id, never the short form");
        assert!(!container.state.is_empty());
        // The M41 bug, asserted against whatever this machine is running: "no healthcheck" is
        // reported by a live daemon as `"none"`, and a badge drawn for it appears on nearly
        // every container in the world.
        assert_ne!(
            container.health.as_deref(),
            Some("none"),
            "`none` means no healthcheck and must never reach the panel as a value"
        );
        // The other M41 bug: a dual-stack publish is reported twice and must collapse.
        let mut seen = Vec::new();
        for port in &container.ports {
            assert!(!seen.contains(&port), "a port is listed once: {port:?}");
            seen.push(port);
        }
    }

    for image in &snapshot.images {
        assert!(
            !image.tags.iter().any(|tag| tag == "<none>:<none>"),
            "an untagged image has no tags rather than a literal <none>:<none>"
        );
    }
}

#[test]
#[ignore = "needs a real Docker daemon"]
fn every_context_in_the_store_parses() {
    // The store is read by cide and not by `docker`, so a spelling this build cannot parse is a
    // context silently missing from the switcher.
    let probes = connect::probe();
    for context in &probes.contexts {
        println!("{:<16} {}", context.name, context.endpoint.as_url());
        assert!(!context.name.is_empty());
    }
}

#[test]
#[ignore = "needs a real Docker daemon with a running container"]
fn a_shell_is_found_and_an_exec_streams_its_output() {
    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");
    let Some(container) = snapshot.containers.iter().find(|c| c.state == "running") else {
        eprintln!("skipped: nothing is running");
        return;
    };
    println!("exec into {} ({})", container.name, container.image);

    // The probe: one exec, and the answer must be a path from the ladder rather than whatever
    // the container happened to print.
    let shell = docker.best_shell(&container.id).expect("a shell is found");
    println!("  shell: {shell}");
    assert!(cide_docker::session::SHELL_LADDER.contains(&shell.as_str()));

    // And a capture, which is the road M44's directory listing takes.
    let printed = docker
        .exec_capture(
            &container.id,
            &["/bin/sh".into(), "-c".into(), "echo cide-ok".into()],
        )
        .expect("a capture runs");
    assert!(printed.contains("cide-ok"), "captured: {printed:?}");
}

#[test]
#[ignore = "needs a real Docker daemon with a running container"]
fn an_exec_session_paints_and_a_log_follow_delivers() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");
    let Some(container) = snapshot.containers.iter().find(|c| c.state == "running") else {
        eprintln!("skipped: nothing is running");
        return;
    };

    let geometry = cide_pty::Geometry::new(80, 24, 8, 17);

    // --- the exec pane -------------------------------------------------------------------
    //
    // The whole of M42's claim in one assertion: bytes from a container reach a `cide-pty` sink,
    // which means they went through the coalescer, the vt100 mirror and the sink list.
    let session = docker
        .exec_session(&container.id, &[], geometry)
        .expect("an exec session opens");

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    session.attach(Arc::new(move |bytes: &[u8]| {
        counter.fetch_add(bytes.len(), Ordering::Relaxed);
        true
    }));

    session.write(b"echo cide-exec-ok\n".to_vec());
    for _ in 0..80 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if seen.load(Ordering::Relaxed) > 0 {
            break;
        }
    }
    let painted = String::from_utf8_lossy(&session.screen_state()).into_owned();
    println!("  exec screen: {:?}", painted.trim());
    assert!(
        painted.contains("cide-exec-ok"),
        "the shell echoed into the vt100 mirror: {painted:?}"
    );

    // `kill` drops the input side, which is the only thing that ends an exec — see
    // `ExecTransport::kill`.
    session.kill();

    // --- the log follow ------------------------------------------------------------------
    let logs = docker
        .logs_session(&container.id, 20, geometry)
        .expect("a log follow opens");
    let bytes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&bytes);
    logs.attach(Arc::new(move |chunk: &[u8]| {
        counter.fetch_add(chunk.len(), Ordering::Relaxed);
        true
    }));
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if bytes.load(Ordering::Relaxed) > 0 {
            break;
        }
    }
    println!("  log bytes: {}", bytes.load(Ordering::Relaxed));
    // Deliberately not asserted non-zero: a container that has printed nothing and prints nothing
    // while this runs is an ordinary state, not a failure. What is asserted is that opening the
    // follow neither errored nor panicked, and that a sink can be attached to it.
    assert!(!logs.has_exited() || bytes.load(Ordering::Relaxed) > 0);
}

#[test]
#[ignore = "needs a real Docker daemon with a running container"]
fn a_container_filesystem_lists_and_a_file_reads() {
    use cide_ipc::docker::ContainerListing;

    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");
    let Some(container) = snapshot.containers.iter().find(|c| c.state == "running") else {
        eprintln!("skipped: nothing is running");
        return;
    };
    println!("browsing {} ({})", container.name, container.image);

    // The root, which every image has and which is the first thing the browser asks for.
    match docker.list_dir(&container.id, "/") {
        ContainerListing::Ready { entries } => {
            println!("  / has {} entries", entries.len());
            assert!(!entries.is_empty(), "a container root is never empty");
            // The parse is only right if the ordinary directories came back as directories.
            let etc = entries
                .iter()
                .find(|e| e.name == "etc")
                .expect("every image has /etc");
            assert!(
                etc.directory,
                "and `ls -p`'s trailing slash was read: {etc:#?}"
            );
            assert_eq!(etc.size, None, "a directory reports no size");
            for entry in entries.iter().take(6) {
                println!(
                    "    {:<24} {}{}",
                    entry.name,
                    if entry.directory { "dir" } else { "file" },
                    entry
                        .link
                        .as_ref()
                        .map(|l| format!(" -> {l}"))
                        .unwrap_or_default()
                );
            }
            assert!(
                entries
                    .iter()
                    .all(|e| !e.name.is_empty() && !e.name.ends_with('/')),
                "no entry keeps its trailing slash or comes back nameless",
            );
            // Directories first — the browser's stable order.
            let first_file = entries.iter().position(|e| !e.directory);
            let last_dir = entries.iter().rposition(|e| e.directory);
            if let (Some(file), Some(dir)) = (first_file, last_dir) {
                assert!(dir < file, "directories sort before files");
            }
        }
        ContainerListing::Unusable { reason } => {
            // A distroless image is an ordinary outcome, not a failure — but it must SAY so.
            println!("  not browsable: {reason}");
            assert!(!reason.is_empty());
            return;
        }
    }

    // And one file, through `GET /archive` — which needs no shell at all.
    let text = docker
        .read_file(&container.id, "/etc/hostname")
        .expect("/etc/hostname reads");
    println!("  /etc/hostname: {:?}", text.trim());
    assert!(!text.is_empty());

    // A directory is refused by name rather than silently opening the first file inside it.
    let refused = docker
        .read_file(&container.id, "/etc")
        .expect_err("a directory is not a file");
    assert!(refused.to_string().contains("directory"), "{refused}");

    // And a binary is refused rather than shown as replacement characters.
    match docker.read_file(&container.id, "/bin/sh") {
        Err(error) => println!("  /bin/sh refused: {error}"),
        Ok(_) => panic!("a binary must not open as text"),
    }
}

#[test]
#[ignore = "needs a real Docker daemon"]
fn a_real_board_carries_no_nulls_for_the_frontend_to_trip_over() {
    // The wire shape, against this machine's actual containers rather than a constructed row.
    // `cide_ipc::docker`'s unit test pins the same claim on values cide chose; this one pins it on
    // whatever the daemon happened to report, which is where the original failure came from — the
    // panel died on `null is not an object` for a container with no compose labels, and every
    // container on the machine it was written on had none.
    let Some(docker) = daemon() else { return };
    let board = docker.snapshot().expect("a live daemon answers");
    let json = serde_json::to_string(&board).expect("serialise");
    // Walked structurally rather than grepped for the substring `null`, and that is not
    // fastidiousness — the first version of this test *did* grep, and failed on a real machine
    // for a reason worth keeping: Docker's `none` network is driven by the **`null` driver**, so
    // `"driver":"null"` is a legitimate string value in every board. A substring check calls that
    // a bug for ever.
    //
    // Naming the offending keys rather than printing a truncated document, for the same reason: a
    // 40 KB board cut at 600 characters is a failure nobody can act on.
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    let mut nulls: Vec<String> = Vec::new();
    fn walk(value: &serde_json::Value, at: &str, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, entry) in map {
                    if entry.is_null() {
                        let path = format!("{at}.{key}");
                        if !out.contains(&path) {
                            out.push(path);
                        }
                    }
                    walk(entry, &format!("{at}.{key}"), out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, &format!("{at}[]"), out);
                }
            }
            _ => {}
        }
    }
    walk(&value, "board", &mut nulls);
    assert!(
        nulls.is_empty(),
        "an absent optional must be an absent key, never null — the frontend's types say the \
         key is missing and a `=== undefined` check is false against null. Still writing null: \
         {nulls:?}"
    );
}

/// Two reads of an unchanged daemon draw the same board, in the same order.
///
/// # Why this needs a real daemon
///
/// Because the defect is in what the daemon *answers*, not in what cide does with it. `GET
/// /networks` and `GET /volumes` return their rows in a different order on every call — six
/// consecutive reads here gave six orders — and nothing synthetic can show that. The panel
/// refetches the whole board on every daemon event, so those two sections reshuffled themselves
/// several times a minute under the user's pointer, which is how it was reported: *"network group
/// rows changing frequently"*.
///
/// `docker network ls` does not have the problem because the **CLI** sorts. cide talks to the API
/// (ADR 0013), so the sort is cide's to do — `files::parse_listing`'s conclusion about `ls`
/// output, one module over.
#[test]
#[ignore = "needs a real Docker daemon"]
fn two_reads_of_an_unchanged_daemon_draw_the_same_order() {
    let Some(docker) = daemon() else { return };

    let names = |snapshot: &cide_ipc::docker::DockerSnapshot| {
        (
            snapshot
                .networks
                .iter()
                .map(|row| row.name.clone())
                .collect::<Vec<_>>(),
            snapshot
                .volumes
                .iter()
                .map(|row| row.name.clone())
                .collect::<Vec<_>>(),
            snapshot
                .containers
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            snapshot
                .images
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
        )
    };

    let first = names(&docker.snapshot().expect("a live daemon answers"));
    // Several, not two: the raw endpoint cycled through orders rather than alternating between
    // them, so a single repeat could agree by luck.
    for attempt in 0..6 {
        let again = names(&docker.snapshot().expect("a live daemon answers"));
        assert_eq!(
            first.0, again.0,
            "the network order moved between reads (attempt {attempt})"
        );
        assert_eq!(
            first.1, again.1,
            "the volume order moved (attempt {attempt})"
        );
        assert_eq!(
            first.2, again.2,
            "the container order moved (attempt {attempt})"
        );
        assert_eq!(
            first.3, again.3,
            "the image order moved (attempt {attempt})"
        );
    }
    println!(
        "stable across 7 reads: {} networks, {} volumes, {} containers, {} images",
        first.0.len(),
        first.1.len(),
        first.2.len(),
        first.3.len()
    );
}

/// What the four surfaces would actually run against a file named on the command line.
///
/// `CIDE_COMPOSE_FILE=<path> cargo test -p cide-docker --test real_daemon -- --ignored
/// what_a_file_would_run --nocapture`
/// The user's own gesture: a compose file goes up, and the board must notice.
///
/// `CIDE_COMPOSE_FILE=<path> cargo test -p cide-docker --test real_daemon -- --ignored
/// a_compose_up_reaches_the_board --nocapture`
///
/// # Why this is end to end and not a unit test
///
/// Because every link in it is somebody else's: Compose creates the objects, the **daemon**
/// decides what to report on `GET /events`, and cide only counts. A test that asserted
/// `watch_events` calls its callback would pass against a daemon that reported nothing.
#[test]
#[ignore = "needs a real daemon and Compose; brings $CIDE_COMPOSE_FILE up and down"]
fn a_compose_up_reaches_the_board() {
    use cide_ipc::docker::ComposeAction;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let Some(file) = std::env::var_os("CIDE_COMPOSE_FILE") else {
        println!("set CIDE_COMPOSE_FILE to a compose file");
        return;
    };
    let path = std::path::PathBuf::from(&file);
    let Some(docker) = daemon() else { return };
    if cide_docker::compose::find_compose().is_none() {
        println!("no compose on this machine");
        return;
    }

    // Down first, so `up` has real work to do and really emits.
    let run = |action| {
        cide_docker::compose::plan(&path, action, &[]).map(|plan| {
            std::process::Command::new(&plan.program)
                .args(&plan.args)
                .current_dir(&plan.working_dir)
                .output()
        })
    };
    let _ = run(ComposeAction::Down);

    let before = docker
        .snapshot()
        .expect("a live daemon answers")
        .containers
        .len();

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let watch = docker.watch_events(move || {
        counter.fetch_add(1, Ordering::AcqRel);
    });
    // The subscription is opened on the runtime's own thread, so give it a moment to be listening
    // before anything is done for it to hear. Without this the test races the socket and the
    // failure would look like "events do not arrive".
    std::thread::sleep(std::time::Duration::from_millis(500));

    run(ComposeAction::Up)
        .expect("a plan")
        .expect("compose ran");
    std::thread::sleep(std::time::Duration::from_secs(2));

    let events = seen.load(Ordering::Acquire);
    let after = docker
        .snapshot()
        .expect("a live daemon answers")
        .containers
        .len();
    println!("containers {before} -> {after}, events seen: {events}");
    assert!(
        events > 0,
        "a compose up emitted no event cide could see — the panel would need a manual Refresh"
    );
    assert!(
        after > before,
        "and the board must actually show the new containers"
    );

    // Dropped, and then nothing more arrives — the half that proves the watch is releasable.
    drop(watch);
    let quiet = seen.load(Ordering::Acquire);
    let _ = run(ComposeAction::Down);
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(
        seen.load(Ordering::Acquire),
        quiet,
        "a dropped watch must stop reporting, or closing the panel changes nothing"
    );
}

/// Write the wire's own `DockerBoard` JSON out, so the webview's adapter can be run against it.
///
/// `CIDE_BOARD_OUT=<path> cargo test -p cide-docker --test real_daemon -- --ignored
/// the_wire_board_as_json --nocapture`
///
/// # Why a real board and not a constructed one
///
/// Because every Docker bug in this feature has been about what a *real daemon* sends: `health`
/// with two spellings for "none", a dual-stack port reported twice, `"driver":"null"` as a
/// legitimate value, and an `#[ts(optional)]` field arriving as `null`. A fixture is written by
/// somebody who already believes they know the shape.
/// A volume's size comes from a filesystem walk, and this is what it costs. (M57)
#[test]
#[ignore = "needs a real Docker daemon; times /system/df, which walks the filesystem"]
fn what_a_volume_size_costs_and_whether_it_is_there() {
    let Some(docker) = daemon() else { return };

    let started = std::time::Instant::now();
    let sizes = docker.volume_usage().expect("a live daemon measures");
    let cold = started.elapsed();

    let started = std::time::Instant::now();
    let again = docker.volume_usage().expect("a live daemon measures");
    let warm = started.elapsed();

    println!(
        "{} volumes measured — first call {:?}, second {:?}",
        sizes.len(),
        cold,
        warm
    );
    assert_eq!(sizes.len(), again.len(), "two reads see the same volumes");

    // The claim the comments make, checked rather than asserted from memory: the plain volume
    // list carries no size, which is *why* this call exists at all.
    let snapshot = docker.snapshot().expect("a live daemon answers");
    assert!(
        !snapshot.volumes.is_empty(),
        "this machine has no volumes; nothing to measure"
    );
    for row in snapshot.volumes.iter().take(3) {
        println!("  {} -> {:?} bytes", row.name, sizes.get(&row.name));
    }

    // And the map is absences, never zeroes: a volume the daemon declined to measure must not
    // read as empty.
    assert!(
        sizes.values().all(|size| *size >= 0),
        "a negative size means the daemon did not measure, and must be absent rather than stored"
    );
}

/// Removing something that is **in use** must refuse, and say what is using it. (M56)
///
/// # Why this needs a real daemon
///
/// Because the refusal is entirely the daemon's. cide passes no `force` and composes no sentence;
/// what makes the feature worth having is that Docker answers `image is being used by stopped
/// container a1b2c3` rather than a bare failure — and nothing synthetic can show that the words
/// arrive, or that they survive `daemon_words`.
///
/// It also pins the half that would be silently wrong: a removal that *succeeded* on something in
/// use would be a data-loss bug with no error anywhere to notice it by.
#[test]
#[ignore = "needs a real Docker daemon; creates a throwaway container, volume and network"]
fn removing_something_in_use_refuses_and_names_what_holds_it() {
    use cide_ipc::docker::Removable;

    let Some(docker) = daemon() else { return };
    let tag = format!("cide-rm-{}", std::process::id());

    // A network and a volume of our own, and a container holding both. `alpine` is already here
    // for the other tests, so nothing is pulled.
    let sh = |args: &[&str]| {
        std::process::Command::new("docker")
            .args(args)
            .output()
            .expect("docker runs")
    };
    sh(&["network", "create", &tag]);
    sh(&["volume", "create", &tag]);
    sh(&[
        "run",
        "-d",
        "--name",
        &tag,
        "--network",
        &tag,
        "-v",
        &format!("{tag}:/data"),
        "alpine",
        "sleep",
        "60",
    ]);

    let cleanup = || {
        sh(&["rm", "-f", &tag]);
        sh(&["volume", "rm", "-f", &tag]);
        sh(&["network", "rm", &tag]);
    };

    // The three refusals, each naming what holds the thing.
    let network_id = {
        let snapshot = docker.snapshot().expect("a live daemon answers");
        snapshot
            .networks
            .iter()
            .find(|row| row.name == tag)
            .map(|row| row.id.clone())
    };
    let image_id = {
        let snapshot = docker.snapshot().expect("a live daemon answers");
        snapshot
            .images
            .iter()
            .find(|row| row.tags.iter().any(|t| t.starts_with("alpine:")))
            .map(|row| row.id.clone())
    };

    let mut checked = 0;
    for (what, holder) in [
        (
            Removable::Volume { name: tag.clone() },
            "a container has it mounted",
        ),
        (
            Removable::Network {
                id: network_id.clone().unwrap_or_default(),
            },
            "a container is attached to it",
        ),
        (
            Removable::Image {
                id: image_id.clone().unwrap_or_default(),
            },
            "a container was made from it",
        ),
    ] {
        let noun = what.noun();
        match docker.remove(&what) {
            Ok(()) => {
                cleanup();
                panic!("removing a {noun} that is in use SUCCEEDED — {holder}");
            }
            Err(error) => {
                let words = error.to_string();
                println!("  {noun}: {words}");
                assert!(
                    !words.trim().is_empty(),
                    "a refusal with no words is worse than none: the user is told the button \
                     failed and not that something is using the {noun}"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 3);

    // And it *succeeds* once nothing holds it — otherwise this test would pass with a `remove`
    // that refuses everything.
    sh(&["rm", "-f", &tag]);
    docker
        .remove(&Removable::Volume { name: tag.clone() })
        .expect("an unused volume removes");
    if let Some(id) = network_id {
        docker
            .remove(&Removable::Network { id })
            .expect("an unused network removes");
    }
    cleanup();
}

#[test]
#[ignore = "diagnostic: writes a live board to $CIDE_BOARD_OUT for the frontend adapter"]
fn the_wire_board_as_json() {
    let Some(out) = std::env::var_os("CIDE_BOARD_OUT") else {
        println!("set CIDE_BOARD_OUT to a path");
        return;
    };
    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");
    let board = cide_ipc::docker::DockerBoard::Ready(Box::new(snapshot));
    let json = serde_json::to_string(&board).expect("a board serialises");
    std::fs::write(&out, &json).expect("write the board");
    println!(
        "wrote {} bytes to {}",
        json.len(),
        std::path::Path::new(&out).display()
    );
}

#[test]
#[ignore = "diagnostic: prints the argv each verb would run for $CIDE_COMPOSE_FILE"]
fn what_a_file_would_run() {
    use cide_ipc::docker::ComposeAction;

    let Some(file) = std::env::var_os("CIDE_COMPOSE_FILE") else {
        println!("set CIDE_COMPOSE_FILE to a compose file");
        return;
    };
    let path = std::path::PathBuf::from(&file);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    println!(
        "{name}: is_compose_file = {}",
        cide_docker::compose::is_compose_file(&name)
    );

    for action in [
        ComposeAction::Up,
        ComposeAction::Down,
        ComposeAction::Restart,
        ComposeAction::Recreate,
        ComposeAction::Build,
        ComposeAction::Pull,
    ] {
        match cide_docker::compose::plan(&path, action, &[]) {
            Ok(plan) => println!(
                "  {:?}: {} {:?}  (cwd {}, title {:?})",
                action, plan.program, plan.args, plan.working_dir, plan.title
            ),
            Err(error) => println!("  {action:?}: refused — {error}"),
        }
    }

    // And one narrowed to a service, which only the editor gutter produces.
    match cide_docker::compose::plan(&path, ComposeAction::Restart, &["web".to_string()]) {
        Ok(plan) => println!("  Restart[web]: {:?}  title {:?}", plan.args, plan.title),
        Err(error) => println!("  Restart[web]: refused — {error}"),
    }
}

#[test]
#[ignore = "diagnostic: prints what this machine's compose lane resolves to"]
fn what_compose_resolves_to() {
    println!("docker binary: {:?}", cide_docker::compose::find_docker());
    println!("invocation: {:?}", cide_docker::compose::find_compose());
    println!("availability: {:?}", cide_docker::compose::availability());
}

/// A real stack, up and down, through the two roads that act on one.
///
/// # Why this test exists
///
/// Because the synthetic ones could not see the bug it was written for. `compose::act` resolved
/// `find_docker()` and built its argv with [`compose::argv`], which deliberately omits the
/// `compose` word — that word belongs to `Invocation::program` and only [`compose::find_compose`]
/// knows whether this rung needs it. So every stack button in the panel ran `docker -p … up` and
/// was refused by the CLI with `unknown shorthand flag: 'p' in -p`, from M43 until M48. Every
/// unit test passed the whole time: each one asserts on the *argv*, and the argv was right.
///
/// The only thing that can catch that class is running the binary, which is what this does.
#[test]
#[ignore = "needs a real Docker daemon and Compose; brings a one-service stack up and down"]
fn a_real_stack_goes_up_and_comes_down_both_ways() {
    use cide_ipc::docker::ComposeAction;

    if cide_docker::compose::find_compose().is_none() {
        println!("no compose on this machine; nothing to prove");
        return;
    }

    // Its own directory, named by pid, so a parallel run cannot collide and so Compose's own
    // project-name derivation (the directory's basename) gives something unique. **Not** the
    // system temp root itself: the working directory *is* the project identity here.
    let dir = std::env::temp_dir().join(format!("cide-compose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    /*
     * **Deliberately not one of Compose's four default names.**
     *
     * This test used `compose.yaml` and therefore could not see the M53 bug: with a default name,
     * Compose finds the file by searching the working directory, so an invocation that passed no
     * `-f` at all worked anyway. A stack brought up from `docker.compose.yaml` answered *"no
     * configuration file provided: not found"* on every panel button, and this test was green the
     * whole time.
     *
     * So the file is named the way the failure needs, and the `-f` below is load-bearing.
     */
    let file = dir.join("docker.compose.yaml");
    // `alpine` with a sleep: no build, no ports to collide on, no volume to leave behind, and it
    // stays up long enough for `down` to have something to stop.
    std::fs::write(
        &file,
        "services:\n  idle:\n    image: alpine\n    command: [\"sleep\", \"120\"]\n",
    )
    .expect("the compose file");

    let project = format!("cide-test-{}", std::process::id());

    // Road one: the panel's. A `-p` from a container label, a working directory, the config files
    // the stack was brought up with, and a wait.
    //
    // **The file list is passed, and with this file name it has to be.** An empty one — which is
    // what the panel sent until M53 — leaves Compose searching the working directory for a
    // default-named file and refusing with "no configuration file provided: not found".
    let files = vec!["docker.compose.yaml".to_string()];
    let up = cide_docker::compose::act(Some(&dir), &project, ComposeAction::Up, &files);
    let up = up.expect("`compose up` must not be refused by the CLI — this is the M48 bug");
    println!("up: {up}");

    /*
     * And the M53 half, asserted rather than assumed: the same call with no files must fail on a
     * file Compose cannot find by name. If this ever starts succeeding, the test above has
     * stopped proving anything.
     *
     * **`Recreate` and not `Restart`, and the difference is the whole shape of the bug.** Compose
     * resolves `restart` and `down` from the *project label* on running containers and needs no
     * config file at all; `up`, `recreate`, `build` and `pull` read the file. So the panel's Stop
     * and Restart worked on a `docker.compose.yaml` stack while Recreate answered "no
     * configuration file provided: not found" — which is precisely how it was reported, and why a
     * probe on the wrong verb passes and proves nothing. Measured, not assumed: `Restart` without
     * `-f` returns `Ok` here.
     */
    let blind = cide_docker::compose::act(Some(&dir), &project, ComposeAction::Recreate, &[]);
    assert!(
        blind.is_err(),
        "a non-default file name must refuse without `-f`, or this test cannot see the bug it \
         was written for: {blind:?}"
    );
    // And it works with them, which is the fix.
    cide_docker::compose::act(Some(&dir), &project, ComposeAction::Recreate, &files)
        .expect("recreate with the stack's own config files must succeed");

    // Road two: the file's. No `-p` at all — Compose derives the project from the directory —
    // and this is what the tree, the gutter and the palette produce.
    let plan = cide_docker::compose::plan(&file, ComposeAction::Recreate, &["idle".to_string()])
        .expect("a plan resolves");
    println!(
        "plan: {} {:?} in {}",
        plan.program, plan.args, plan.working_dir
    );
    assert!(
        plan.args.contains(&"--force-recreate".to_string()),
        "recreate must force it: {:?}",
        plan.args
    );
    assert_eq!(plan.args.last().map(String::as_str), Some("idle"));
    // Run the plan exactly as a pane would, and assert the CLI accepts the argv. A pane would
    // spawn it on a pty; here the exit status is the whole question.
    let ran = std::process::Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .output()
        .expect("the plan's program runs");
    println!(
        "plan ran: ok={} stderr={}",
        ran.status.success(),
        String::from_utf8_lossy(&ran.stderr).trim()
    );
    assert!(
        ran.status.success(),
        "the gutter's own argv must be one Compose accepts: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    // And down, both projects — the `-p` one and the directory-derived one the plan made.
    let down = cide_docker::compose::act(Some(&dir), &project, ComposeAction::Down, &files);
    println!("down: {down:?}");
    // Through `plan` again rather than by hand: the leading `compose` word is `Invocation`'s
    // private business, and reaching around it here is how the M48 bug was written in the first
    // place.
    if let Ok(tidy) = cide_docker::compose::plan(&file, ComposeAction::Down, &[]) {
        let _ = std::process::Command::new(&tidy.program)
            .args(&tidy.args)
            .current_dir(&tidy.working_dir)
            .output();
    }
    std::fs::remove_dir_all(&dir).ok();
    down.expect("`compose down` must not be refused either");
}

#[test]
#[ignore = "needs a real Docker daemon with a running container"]
fn a_detail_reads_for_every_kind_on_this_machine() {
    use cide_ipc::docker::{DockerDetail, InspectTarget};

    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");

    if let Some(container) = snapshot.containers.first() {
        match docker.detail(&InspectTarget::Container {
            id: container.id.clone(),
        }) {
            DockerDetail::Container(detail) => {
                println!(
                    "container {} — {} port(s), {} mount(s), {} env, {} label(s), networks {:?}",
                    detail.name,
                    detail.ports.len(),
                    detail.mounts.len(),
                    detail.env.len(),
                    detail.labels.len(),
                    detail.networks
                );
                assert_eq!(detail.id.len(), 64, "the full id");
                assert!(!detail.image.is_empty());
                // Sorted, because the daemon's map order differs between reads and an unsorted
                // pane reshuffles on every refresh.
                let names: Vec<&str> = detail.labels.iter().map(|p| p.name.as_str()).collect();
                let mut sorted = names.clone();
                sorted.sort_unstable();
                assert_eq!(names, sorted, "labels are sorted");
            }
            other => panic!("expected a container detail, got {other:?}"),
        }
    }

    if let Some(image) = snapshot.images.first() {
        let detail = docker.detail(&InspectTarget::Image {
            id: image.id.clone(),
        });
        assert!(matches!(detail, DockerDetail::Image(_)), "{detail:?}");
    }
    if let Some(volume) = snapshot.volumes.first() {
        let detail = docker.detail(&InspectTarget::Volume {
            name: volume.name.clone(),
        });
        assert!(matches!(detail, DockerDetail::Volume(_)), "{detail:?}");
    }
    if let Some(network) = snapshot.networks.first() {
        let detail = docker.detail(&InspectTarget::Network {
            id: network.id.clone(),
        });
        assert!(matches!(detail, DockerDetail::Network(_)), "{detail:?}");
    }

    // A thing that is gone is a *shape*, not an error — it is the ordinary outcome of clicking a
    // row for a container that was removed a moment ago.
    let gone = docker.detail(&InspectTarget::Container { id: "0".repeat(64) });
    assert!(matches!(gone, DockerDetail::Missing { .. }), "{gone:?}");
}

#[test]
#[ignore = "needs a real Docker daemon; pauses and unpauses a container to produce an event"]
fn the_event_stream_reports_a_change_and_stops_when_dropped() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let Some(docker) = daemon() else { return };
    let snapshot = docker.snapshot().expect("a live daemon answers");
    let Some(container) = snapshot.containers.iter().find(|c| c.state == "running") else {
        eprintln!("skipped: nothing is running");
        return;
    };

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let watch = docker.watch_events(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    // The subscription is established asynchronously; give it a moment before provoking one.
    std::thread::sleep(std::time::Duration::from_millis(400));

    // Pause and unpause: the least invasive pair that produces real events, and it leaves the
    // container exactly as it was found.
    docker
        .act(&container.id, cide_ipc::docker::ContainerAction::Pause)
        .expect("pause");
    docker
        .act(&container.id, cide_ipc::docker::ContainerAction::Unpause)
        .expect("unpause");

    for _ in 0..40 {
        if seen.load(Ordering::Relaxed) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let during = seen.load(Ordering::Relaxed);
    println!("  events while watching: {during}");
    assert!(during > 0, "the daemon's events reached the callback");

    // Dropping the watch must stop it. A subscription that outlived its connection would go on
    // reporting the *old* daemon's events after an endpoint switch.
    drop(watch);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let after_drop = seen.load(Ordering::Relaxed);
    docker
        .act(&container.id, cide_ipc::docker::ContainerAction::Pause)
        .expect("pause");
    docker
        .act(&container.id, cide_ipc::docker::ContainerAction::Unpause)
        .expect("unpause");
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(
        seen.load(Ordering::Relaxed),
        after_drop,
        "a dropped watch reports nothing further"
    );
}
