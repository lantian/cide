//! Linux packaging: AppImage, .deb, and a Flatpak manifest.
//!
//! # Why AppImage is the primary channel and not one of three equals
//!
//! `tauri-plugin-updater` supports exactly one Linux format: AppImage. That is not a
//! preference we are expressing, it is the whole of the plugin's Linux support, and it
//! decides the distribution story for an application whose central dependency — the `claude`
//! CLI — updates itself every few days. A build that cannot update itself will be running
//! against a CLI several minor versions ahead of the protocol constants it was compiled
//! with. So AppImage is what a user is pointed at, `.deb` is the convenience for people who
//! want their package manager to own it, and Flatpak is the awkward one (see below and
//! `docs/adr/0007`).
//!
//! # This task does not build anything unless asked
//!
//! Bundling needs a release build of the whole workspace, a produced `ui/dist`, and
//! `cargo-tauri`; on this machine that is a twenty-minute job, and running it as a side
//! effect of a task somebody typed to see what it would do is hostile. The default is a
//! preflight and a printed plan. `--run` executes it.
//!
//! # The Flatpak manifest is generated, not hand-maintained
//!
//! It restates facts that already live in `tauri.conf.json` and `Cargo.toml` — the app id,
//! the version, the binary names. A hand-written copy drifts from them silently and the
//! failure surfaces as a package that installs and then cannot find its own executable. So
//! it is generated, checked in, and `--check` fails when the checked-in copy is stale, in
//! the same shape as `codegen --check`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The Tauri configuration, which is where the identifier, version and bundle targets live.
const TAURI_CONF: &str = "crates/cide-app/tauri.conf.json";

/// Where the generated Flatpak files are checked in.
const FLATPAK_DIR: &str = "packaging/flatpak";

/// The directory `cargo tauri build` must run from — the crate holding `tauri.conf.json`.
const APP_CRATE: &str = "crates/cide-app";

/// The Flatpak runtime this manifest targets.
///
/// Verified against `flatpak remote-ls flathub --runtime` on 2026-08-09, which offered
/// `org.gnome.Platform` 49 and 50 and nothing older. 49 rather than 50 because a runtime
/// that has been out longer has had longer for its WebKitGTK to settle, and this application
/// is more exposed to WebKitGTK than to anything else in the stack.
///
/// **Unverified and load-bearing:** that this runtime carries the WebKitGTK **4.1** (GTK 3)
/// ABI that Tauri 2 links, rather than only 6.0 (GTK 4). Confirming it needs the runtime
/// installed, which is a multi-gigabyte download this task will not perform on its own. See
/// `docs/adr/0007`; the preflight checks that the version still exists, which is the part it
/// can check honestly.
const GNOME_RUNTIME: &str = "49";

/// Which artefacts to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub appimage: bool,
    pub deb: bool,
    pub flatpak: bool,
}

impl Targets {
    /// Everything, which is what naming no target means.
    pub const ALL: Self = Self {
        appimage: true,
        deb: true,
        flatpak: true,
    };

    /// The `--bundles` value for `cargo tauri build`, or `None` when neither Tauri target
    /// was asked for.
    fn bundles(self) -> Option<String> {
        let mut names = Vec::new();
        if self.appimage {
            names.push("appimage");
        }
        if self.deb {
            names.push("deb");
        }
        (!names.is_empty()).then(|| names.join(","))
    }
}

/// What the task was asked to do.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub targets: Targets,
    /// Rewrite the checked-in Flatpak manifest.
    pub write: bool,
    /// Fail if the checked-in Flatpak manifest is stale. A CI gate.
    pub check: bool,
    /// Actually invoke the bundlers.
    pub run: bool,
}

/// A preflight result. Warnings do not stop a build; failures do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok(String),
    Warn(String),
    Fail(String),
}

impl Verdict {
    fn marker(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok  ",
            Self::Warn(_) => "warn",
            Self::Fail(_) => "FAIL",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Ok(d) | Self::Warn(d) | Self::Fail(d) => d,
        }
    }
}

/// The facts about this repository that the plan and the manifest are built from.
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub identifier: String,
    pub version: String,
    pub product_name: String,
    pub bundle_targets: Vec<String>,
    pub icons: Vec<String>,
    pub has_updater: bool,
}

pub fn package(root: &Path, opts: Options) -> Result<()> {
    let info = read_app_info(root)?;

    if opts.check {
        return check_generated(root, &info);
    }
    if opts.write {
        return write_generated(root, &info);
    }

    let checks = preflight(root, &info, opts.targets);
    println!("preflight for {} {}", info.product_name, info.version);
    for check in &checks {
        println!("  [{}] {}", check.marker(), check.detail());
    }
    let failures: Vec<&Verdict> = checks
        .iter()
        .filter(|c| matches!(c, Verdict::Fail(_)))
        .collect();

    let steps = plan(&info, opts.targets);
    println!("\nplan:");
    for step in &steps {
        println!("  $ {}", step.display());
    }

    if !failures.is_empty() {
        bail!(
            "{} preflight check(s) failed; nothing was built",
            failures.len()
        );
    }

    if !opts.run {
        println!(
            "\nNothing was built. Re-run with `--run` to execute the plan, or copy the \
             commands above.\nSee docs/adr/0007-packaging-and-distribution.md."
        );
        return Ok(());
    }

    for step in steps {
        step.execute(root)?;
    }
    println!("\npackaging complete; artefacts are under target/release/bundle/");
    Ok(())
}

/// One command in the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    program: String,
    args: Vec<String>,
    /// Relative to the workspace root.
    cwd: String,
}

impl Step {
    fn display(&self) -> String {
        let mut line = String::new();
        if self.cwd != "." {
            line.push_str(&format!("cd {} && ", self.cwd));
        }
        line.push_str(&self.program);
        for arg in &self.args {
            line.push(' ');
            // Quoted only where it matters, so the printed line can be pasted into a shell.
            if arg.contains(' ') {
                line.push_str(&format!("\"{arg}\""));
            } else {
                line.push_str(arg);
            }
        }
        line
    }

    fn execute(&self, root: &Path) -> Result<()> {
        println!("\n$ {}", self.display());
        let status = Command::new(&self.program)
            .args(&self.args)
            .current_dir(root.join(&self.cwd))
            .status()
            .with_context(|| format!("running `{}`", self.display()))?;
        if !status.success() {
            bail!("`{}` failed with {status}", self.display());
        }
        Ok(())
    }
}

/// The commands that would produce the requested targets.
pub fn plan(info: &AppInfo, targets: Targets) -> Vec<Step> {
    let mut steps = Vec::new();

    if let Some(bundles) = targets.bundles() {
        // Run from the app crate: `cargo tauri build` finds `tauri.conf.json` by walking up
        // from the working directory, and this workspace has no `src-tauri`.
        //
        // `beforeBuildCommand` in that config runs `pnpm build` in `ui/`, so the frontend is
        // not a separate step here — adding one would build it twice.
        steps.push(Step {
            program: "cargo".into(),
            args: vec!["tauri".into(), "build".into(), "--bundles".into(), bundles],
            cwd: APP_CRATE.into(),
        });
    }

    if targets.flatpak {
        steps.push(Step {
            program: "flatpak-builder".into(),
            args: vec![
                "--force-clean".into(),
                // Installed into the user's own installation rather than published to a
                // repo: this task's job is a package you can run on this machine, and
                // publishing needs signing keys it must not go looking for.
                //
                // The build needs the network — cargo fetches crates, npm fetches packages —
                // which Flathub's own builders forbid. See docs/adr/0007.
                "--install".into(),
                "--user".into(),
                format!("target/flatpak/{}", info.identifier),
                manifest_path(info),
            ],
            cwd: ".".into(),
        });
    }

    steps
}

/// The manifest's path, relative to the workspace root.
fn manifest_path(info: &AppInfo) -> String {
    format!("{FLATPAK_DIR}/{}.yml", info.identifier)
}

/// Everything that can be checked without building.
pub fn preflight(root: &Path, info: &AppInfo, targets: Targets) -> Vec<Verdict> {
    let mut out = Vec::new();

    for (wanted, name) in [(targets.appimage, "appimage"), (targets.deb, "deb")] {
        if !wanted {
            continue;
        }
        if info.bundle_targets.iter().any(|t| t == name) {
            out.push(Verdict::Ok(format!("{TAURI_CONF} bundles `{name}`")));
        } else {
            out.push(Verdict::Fail(format!(
                "{TAURI_CONF} does not list `{name}` in bundle.targets, so \
                 `cargo tauri build` will not produce one"
            )));
        }
    }

    for icon in &info.icons {
        let path = root.join(APP_CRATE).join(icon);
        if path.exists() {
            out.push(Verdict::Ok(format!("icon {icon}")));
        } else {
            out.push(Verdict::Fail(format!(
                "icon {icon} is named in {TAURI_CONF} but missing at {}",
                path.display()
            )));
        }
    }

    if targets.appimage || targets.deb {
        out.push(match which("cargo-tauri") {
            Some(path) => Verdict::Ok(format!("cargo-tauri at {}", path.display())),
            None => Verdict::Fail(
                "cargo-tauri is not on PATH — install it with `cargo install tauri-cli \
                 --version ^2`"
                    .into(),
            ),
        });
    }

    if targets.appimage {
        out.push(if info.has_updater {
            Verdict::Ok("the updater plugin is configured".into())
        } else {
            // A warning, not a failure: an AppImage without an updater endpoint is a
            // perfectly good AppImage. It is just not the *self-updating* channel, which is
            // the only reason this format was chosen over the others.
            Verdict::Warn(
                "no `plugins.updater` in the config: this AppImage will not self-update, \
                 and AppImage is the only Linux format `tauri-plugin-updater` supports"
                    .into(),
            )
        });
    }

    if targets.flatpak {
        out.push(flatpak_runtime_check());
        out.push(match which("flatpak-builder") {
            Some(path) => Verdict::Ok(format!("flatpak-builder at {}", path.display())),
            None => Verdict::Warn(
                "flatpak-builder is not on PATH; the manifest can still be generated and \
                 checked, but not built"
                    .into(),
            ),
        });
        for (relative, expected) in generated(info) {
            let path = root.join(&relative);
            out.push(match fs::read_to_string(&path) {
                Ok(current) if current == expected => Verdict::Ok(format!("{relative} in sync")),
                Ok(_) => Verdict::Fail(format!(
                    "{relative} is stale — run `cargo xtask package --write`"
                )),
                Err(_) => Verdict::Fail(format!(
                    "{relative} is missing — run `cargo xtask package --write`"
                )),
            });
        }
    }

    out
}

/// Ask flathub whether the runtime this manifest pins still exists.
///
/// Skipped rather than failed when `flatpak` is absent: the manifest is checked into a
/// repository that people build on machines with no Flatpak at all, and a red preflight for
/// a target they did not ask about teaches them to ignore preflights.
fn flatpak_runtime_check() -> Verdict {
    if which("flatpak").is_none() {
        return Verdict::Warn(
            "flatpak is not installed, so the pinned runtime version could not be checked".into(),
        );
    }
    let output = Command::new("flatpak")
        .args(["remote-ls", "flathub", "--runtime", "--columns=ref"])
        .output();
    let Ok(output) = output else {
        return Verdict::Warn("could not query flathub for available runtimes".into());
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    // `--columns=ref` prints a full ref: `runtime/org.gnome.Platform/x86_64/49`. Matched on
    // the tail rather than the whole line so this keeps working on aarch64.
    let wanted = format!("/org.gnome.Platform/{}/{GNOME_RUNTIME}", arch());
    if listing.lines().any(|line| line.trim().ends_with(&wanted)) {
        Verdict::Ok(format!(
            "flathub offers org.gnome.Platform//{GNOME_RUNTIME}"
        ))
    } else {
        Verdict::Warn(format!(
            "flathub does not currently list org.gnome.Platform//{GNOME_RUNTIME}; the runtime \
             pinned in xtask/src/package.rs needs revisiting"
        ))
    }
}

/// Flatpak's name for this machine's architecture, which is not always Rust's.
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}

/// Find an executable on `PATH`.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

// --- the generated Flatpak files -----------------------------------------------------------

/// Every file this task owns, as (path relative to the workspace root, contents).
///
/// One list so that writing, checking and the preflight cannot disagree about what exists.
/// The desktop entry and the AppStream metainfo are here rather than hand-written for the
/// same reason as the manifest: all three restate the app id and version, and a package whose
/// `.desktop` names a different id than its manifest installs and then does not appear in any
/// launcher, with nothing failing.
pub fn generated(info: &AppInfo) -> Vec<(String, String)> {
    vec![
        (manifest_path(info), flatpak_manifest(info)),
        (
            format!("{FLATPAK_DIR}/{}.desktop", info.identifier),
            desktop_entry(info),
        ),
        (
            format!("{FLATPAK_DIR}/{}.metainfo.xml", info.identifier),
            metainfo(info),
        ),
    ]
}

fn write_generated(root: &Path, info: &AppInfo) -> Result<()> {
    for (relative, contents) in generated(info) {
        let path = root.join(&relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context(format!("creating {}", parent.display()))?;
        }
        fs::write(&path, contents).context(format!("writing {}", path.display()))?;
        println!("package: wrote {relative}");
    }
    Ok(())
}

fn check_generated(root: &Path, info: &AppInfo) -> Result<()> {
    let mut stale = Vec::new();
    for (relative, expected) in generated(info) {
        match fs::read_to_string(root.join(&relative)) {
            Ok(current) if current == expected => {}
            Ok(_) => stale.push(format!("  changed  {relative}")),
            Err(_) => stale.push(format!("  missing  {relative}")),
        }
    }
    if !stale.is_empty() {
        bail!(
            "the packaging files are out of date — run `cargo xtask package --write`\n{}\n\
             They restate the app id, version and binary names from {TAURI_CONF}. A copy that \
             has drifted from them produces a package that installs and then cannot find its \
             own executable.",
            stale.join("\n")
        );
    }
    println!("package: {} packaging files in sync", generated(info).len());
    Ok(())
}

/// The launcher entry.
///
/// `StartupWMClass` matters more here than it looks: the window is undecorated and its
/// application id comes from GTK, so without this a taskbar groups cide under a generic
/// entry and shows no icon.
fn desktop_entry(info: &AppInfo) -> String {
    format!(
        "\
[Desktop Entry]
Type=Application
Name={product}
GenericName=IDE
Comment=A Claude-Code-native IDE
Exec={command} %U
Icon={id}
Terminal=false
Categories=Development;IDE;
Keywords=claude;ide;editor;terminal;git;
StartupNotify=true
StartupWMClass={command}
",
        product = info.product_name,
        command = info.product_name,
        id = info.identifier,
    )
}

/// AppStream metadata.
///
/// Required by Flathub and read by GNOME Software and Discover. `<launchable>` has to name
/// the desktop file exactly, or the store shows the app and offers no way to start it.
fn metainfo(info: &AppInfo) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- GENERATED FILE — DO NOT EDIT. Written by `cargo xtask package --write`. -->
<component type="desktop-application">
  <id>{id}</id>
  <name>{product}</name>
  <summary>A Claude-Code-native IDE</summary>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>MIT OR Apache-2.0</project_license>
  <launchable type="desktop-id">{id}.desktop</launchable>
  <description>
    <p>
      An IDE built around a live Claude Code session: a pinned project console of tiling
      Claude and shell panes, alongside a file tree, tabbed editor and an IDEA-style git
      commit tool window.
    </p>
    <p>
      cide drives the Claude Code CLI, which it expects to find on the host. It does not
      bundle one: the CLI updates itself, and a copy frozen inside a package would fall
      behind the protocol it speaks.
    </p>
  </description>
  <releases>
    <release version="{version}"/>
  </releases>
</component>
"#,
        id = info.identifier,
        product = info.product_name,
        version = info.version,
    )
}

/// The Flatpak manifest for this build.
///
/// Deterministic: the same `AppInfo` always yields the same bytes, which is what lets
/// `--check` be a gate.
pub fn flatpak_manifest(info: &AppInfo) -> String {
    format!(
        r#"# GENERATED FILE — DO NOT EDIT.
#
# Written by `cargo xtask package --write`; `cargo xtask package --check` fails when this
# copy has drifted from {conf}. Edit the generator in xtask/src/package.rs.
#
# Read docs/adr/0007-packaging-and-distribution.md before using this. Two things about it
# are unlike the AppImage and .deb channels:
#
#  1. cide's central feature is spawning the `claude` CLI, which lives on the *host* and
#     updates itself. A sandboxed app has neither. `--filesystem=host` and the
#     `org.freedesktop.Flatpak` talk-name below are what let it reach one; without both,
#     every Claude pane fails to spawn and the product does not work.
#  2. This manifest builds with network access, which Flathub's own builders forbid. A
#     Flathub submission additionally needs offline source manifests generated by
#     flatpak-builder-tools (flatpak-cargo-generator.py and flatpak-node-generator).
#
# libgit2 and OpenSSL are deliberately absent as modules: `git2` is built with its
# `vendored-libgit2` and `vendored-openssl` features (see the workspace Cargo.toml), so both
# are compiled into the binary. Taking them from the runtime instead would tie the package to
# whatever ABI that runtime ships and reintroduce exactly the class of breakage vendoring
# exists to avoid. The cost is a longer build and a perl dependency for OpenSSL's own build
# scripts, which the SDK provides.

app-id: {id}
runtime: org.gnome.Platform
runtime-version: '{runtime}'
sdk: org.gnome.Sdk
sdk-extensions:
  - org.freedesktop.Sdk.Extension.rust-stable
  - org.freedesktop.Sdk.Extension.node22
command: {command}

finish-args:
  # Wayland first, X11 as the fallback. Never `--socket=x11` alone: this application draws
  # its own window decorations and its resize grips were built against a Wayland compositor.
  - --socket=wayland
  - --socket=fallback-x11
  - --share=ipc
  # Terminals and the editor are GPU paths. Without dri the webview falls back to a software
  # rasteriser, which the graphics ladder cannot rescue.
  - --device=dri
  # The IDE MCP server and the hook socket are both loopback-only, but `claude` itself talks
  # to the network.
  - --share=network
  # The whole point of the app is opening the user's projects, and it also has to reach
  # ~/.claude for the CLI's own lockfile directory.
  - --filesystem=host
  # Lets the app run `claude` on the host with `flatpak-spawn --host`. Without it the CLI is
  # simply not present: it is not a dependency this package can bundle, because it updates
  # itself on the user's machine.
  - --talk-name=org.freedesktop.Flatpak

modules:
  - name: cide
    buildsystem: simple
    build-options:
      append-path: /usr/lib/sdk/rust-stable/bin:/usr/lib/sdk/node22/bin
      env:
        CARGO_HOME: /run/build/cide/cargo
    build-commands:
      - npm --prefix ui ci
      - npm --prefix ui run build
      - cargo build --release --locked -p cide-app -p cide-hook -p cide-headless
      - install -Dm755 target/release/cide /app/bin/{command}
      # cide-hook must sit beside the main binary: `cmd::session::hook_settings` locates it
      # relative to `current_exe`, because the child's cwd is the project root and its PATH
      # is the user's.
      - install -Dm755 target/release/cide-hook /app/bin/cide-hook
      - install -Dm755 target/release/cide-headless /app/bin/cide-headless
      - install -Dm644 crates/cide-app/icons/128x128.png /app/share/icons/hicolor/128x128/apps/{id}.png
      - install -Dm644 packaging/flatpak/{id}.desktop /app/share/applications/{id}.desktop
      - install -Dm644 packaging/flatpak/{id}.metainfo.xml /app/share/metainfo/{id}.metainfo.xml
    sources:
      - type: dir
        path: ../..
"#,
        conf = TAURI_CONF,
        id = info.identifier,
        runtime = GNOME_RUNTIME,
        command = info.product_name,
    )
}

// --- reading the repository ---------------------------------------------------------------

/// Read the identity and bundle configuration out of `tauri.conf.json`.
pub fn read_app_info(root: &Path) -> Result<AppInfo> {
    let path = root.join(TAURI_CONF);
    let text = fs::read_to_string(&path).context(format!("reading {}", path.display()))?;
    let conf: serde_json::Value =
        serde_json::from_str(&text).context(format!("{} is not valid JSON", path.display()))?;

    let string = |value: Option<&serde_json::Value>, what: &str| -> Result<String> {
        value
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("{} has no string `{what}`", path.display()))
    };

    let list = |value: Option<&serde_json::Value>| -> Vec<String> {
        value
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };

    Ok(AppInfo {
        identifier: string(conf.get("identifier"), "identifier")?,
        version: string(conf.get("version"), "version")?,
        product_name: string(conf.get("productName"), "productName")?,
        bundle_targets: list(conf.pointer("/bundle/targets")),
        icons: list(conf.pointer("/bundle/icon")),
        has_updater: conf.pointer("/plugins/updater").is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> AppInfo {
        AppInfo {
            identifier: "dev.cide.ide".into(),
            version: "0.1.0".into(),
            product_name: "cide".into(),
            bundle_targets: vec!["appimage".into(), "deb".into()],
            icons: vec!["icons/32x32.png".into()],
            has_updater: false,
        }
    }

    #[test]
    fn the_real_config_is_readable() {
        // The generator restates this file's contents; a rename in it must fail here rather
        // than produce a manifest naming a binary that does not exist.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("tauri.conf.json parses");
        assert_eq!(info.identifier, "dev.cide.ide");
        assert!(info.bundle_targets.contains(&"appimage".to_string()));
        assert!(!info.product_name.is_empty());
    }

    #[test]
    fn the_manifest_is_byte_stable() {
        // `--check` is only a gate if a no-op run produces identical bytes.
        assert_eq!(flatpak_manifest(&info()), flatpak_manifest(&info()));
    }

    #[test]
    fn the_manifest_names_the_real_binaries_and_id() {
        let manifest = flatpak_manifest(&info());
        assert!(manifest.contains("app-id: dev.cide.ide"));
        assert!(manifest.contains("/app/bin/cide-hook"));
        assert!(manifest.contains("command: cide"));
    }

    #[test]
    fn the_manifest_vendors_rather_than_taking_libgit2_from_the_runtime() {
        // Adding a libgit2 or openssl module would silently take precedence over the
        // vendored build and tie the package to the runtime's ABI.
        let manifest = flatpak_manifest(&info());
        assert!(
            !manifest.contains("name: libgit2"),
            "libgit2 is vendored; it must not also be a module"
        );
        assert!(!manifest.contains("name: openssl"));
    }

    #[test]
    fn the_flatpak_can_reach_the_host_cli() {
        // Without both of these the app installs, launches, and then fails to spawn a single
        // Claude pane — the one failure mode that makes this channel worthless.
        let manifest = flatpak_manifest(&info());
        assert!(manifest.contains("--filesystem=host"));
        assert!(manifest.contains("--talk-name=org.freedesktop.Flatpak"));
    }

    #[test]
    fn the_tauri_bundle_list_is_one_command() {
        let steps = plan(&info(), Targets::ALL);
        let tauri: Vec<&Step> = steps.iter().filter(|s| s.program == "cargo").collect();
        assert_eq!(tauri.len(), 1, "the bundler must not be invoked twice");
        assert!(tauri[0].args.contains(&"appimage,deb".to_string()));
        assert_eq!(tauri[0].cwd, APP_CRATE);
    }

    #[test]
    fn asking_for_flatpak_alone_does_not_invoke_the_tauri_bundler() {
        let targets = Targets {
            appimage: false,
            deb: false,
            flatpak: true,
        };
        assert_eq!(targets.bundles(), None);
        let steps = plan(&info(), targets);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].program, "flatpak-builder");
    }

    #[test]
    fn the_generated_set_is_the_manifest_plus_its_two_companions() {
        let paths: Vec<String> = generated(&info()).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            [
                "packaging/flatpak/dev.cide.ide.yml",
                "packaging/flatpak/dev.cide.ide.desktop",
                "packaging/flatpak/dev.cide.ide.metainfo.xml",
            ]
        );
    }

    #[test]
    fn the_manifest_installs_exactly_the_files_it_generates() {
        // The manifest's `install -Dm644 packaging/flatpak/…` lines name the desktop entry
        // and the metainfo. If `generated` stopped writing one, the build would fail deep
        // inside flatpak-builder with a missing-file error naming a path nobody recognises.
        let info = info();
        let manifest = flatpak_manifest(&info);
        for (path, _) in generated(&info) {
            if path.ends_with(".yml") {
                continue;
            }
            assert!(
                manifest.contains(&path),
                "the manifest never installs {path}"
            );
        }
    }

    #[test]
    fn the_desktop_entry_and_the_metainfo_agree_on_the_id() {
        // A `.desktop` whose name does not match `<launchable>` installs cleanly and then
        // does not appear in any launcher.
        let info = info();
        assert!(metainfo(&info).contains("<launchable type=\"desktop-id\">dev.cide.ide.desktop"));
        assert!(desktop_entry(&info).contains("Icon=dev.cide.ide"));
    }

    #[test]
    fn a_missing_bundle_target_is_a_failure_not_a_warning() {
        let mut info = info();
        info.bundle_targets = vec!["deb".into()];
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::ALL);
        assert!(
            checks
                .iter()
                .any(|c| matches!(c, Verdict::Fail(d) if d.contains("appimage"))),
            "{checks:?}"
        );
    }

    #[test]
    fn a_missing_updater_is_a_warning_not_a_failure() {
        // An AppImage with no update endpoint is still a working AppImage.
        let checks = preflight(Path::new("/nonexistent"), &info(), Targets::ALL);
        assert!(
            checks
                .iter()
                .any(|c| matches!(c, Verdict::Warn(d) if d.contains("self-update"))),
            "{checks:?}"
        );
    }

    #[test]
    fn a_printed_step_is_pasteable() {
        let step = Step {
            program: "cargo".into(),
            args: vec!["tauri".into(), "build".into()],
            cwd: APP_CRATE.into(),
        };
        assert_eq!(step.display(), "cd crates/cide-app && cargo tauri build");
    }
}
