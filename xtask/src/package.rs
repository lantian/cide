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
//! # `cide-hook` is a second binary, and the bundler does not know about it
//!
//! `cargo tauri build` bundles the `[[bin]]` targets of the crate holding `tauri.conf.json`
//! and nothing else — `tauri-cli`'s `get_binaries` reads that one `Cargo.toml` plus its
//! `src/bin/`. `cide-hook` is a separate workspace crate (it must be: it may not link tauri),
//! so left alone the bundler ships an AppImage containing only `cide`. That package launches,
//! looks correct, and every Claude session inside it runs with no hooks — no token figures, no
//! fast buffer reload, and a close confirm that cannot tell busy from idle. Nothing errors;
//! the features are simply absent.
//!
//! So the hook rides along as a Tauri *sidecar*: `bundle.externalBin` in `tauri.conf.json`
//! names `../../target/release/cide-hook`, the bundler looks for that path with the target
//! triple appended, and `Settings::copy_binaries` strips the triple back off when it copies
//! it into `usr/bin/` — beside `cide`, which is exactly where `cmd::session::hook_settings`
//! looks (`current_exe().parent().join("cide-hook")`). Two steps in the plan produce that
//! suffixed copy. The alternative — `bundle.linux.appimage.files` — was rejected because it
//! is AppImage-only, so the `.deb` would have silently kept shipping without the hook.
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

/// The second binary. See the module docs: without it in the bundle the product runs with
/// no hooks and nothing reports an error.
const HOOK_BIN: &str = "cide-hook";

/// Where the sidecar copy is written, relative to the workspace root. `tauri.conf.json`'s
/// `externalBin` entry names the same path relative to `crates/cide-app`, and the two have to
/// agree; `the_sidecar_path_matches_the_checked_in_config` asserts they do.
const HOOK_SIDECAR_DIR: &str = "target/release";

/// The `cargo-tauri` major version this configuration requires.
///
/// `tauri.conf.json` is a v2 schema and the workspace links tauri 2.x. A v1 `cargo-tauri`
/// on `PATH` is the likely accident — it is what `cargo install tauri-cli` gave people for
/// years — and it fails on this config with a schema error a long way from its cause, so the
/// preflight names the version rather than only checking that *something* is installed.
const TAURI_CLI_MAJOR: u64 = 2;

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
    /// `bundle.externalBin` — the sidecars the bundler copies beside the main binary.
    pub external_bin: Vec<String>,
}

impl AppInfo {
    /// Whether `bundle.externalBin` carries `cide-hook`.
    ///
    /// Matched on the file name so a change of directory in the config does not silently
    /// turn this check off.
    fn bundles_the_hook(&self) -> bool {
        self.external_bin
            .iter()
            .any(|p| Path::new(p).file_name().is_some_and(|n| n == HOOK_BIN))
    }
}

pub fn package(root: &Path, opts: Options) -> Result<()> {
    let info = read_app_info(root)?;

    if opts.check {
        return check_generated(root, &info);
    }
    if opts.write {
        return write_generated(root, &info);
    }

    let triple = host_triple()?;
    let checks = preflight(root, &info, opts.targets);
    println!("preflight for {} {}", info.product_name, info.version);
    for check in &checks {
        println!("  [{}] {}", check.marker(), check.detail());
    }
    let failures: Vec<&Verdict> = checks
        .iter()
        .filter(|c| matches!(c, Verdict::Fail(_)))
        .collect();

    let steps = plan(root, &info, opts.targets, &triple);
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
    for (path, bytes) in artefacts(root) {
        println!("  {path}  {:.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    }
    Ok(())
}

/// The bundles that exist under `target/release/bundle`, with their sizes.
///
/// Printed after a build because "it succeeded" is not the interesting part — a bundle that
/// came out at 12 MiB has not picked up WebKit's helper processes, and the number is the
/// cheapest way to notice.
fn artefacts(root: &Path) -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let bundle = root.join("target/release/bundle");
    for sub in ["appimage", "deb"] {
        let Ok(entries) = fs::read_dir(bundle.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_artefact = path
                .extension()
                .is_some_and(|e| e == "AppImage" || e == "deb");
            if is_artefact && let Ok(meta) = entry.metadata() {
                found.push((
                    format!(
                        "target/release/bundle/{sub}/{}",
                        entry.file_name().to_string_lossy()
                    ),
                    meta.len(),
                ));
            }
        }
    }
    found.sort();
    found
}

/// One command in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    program: String,
    args: Vec<String>,
    /// Relative to the workspace root.
    cwd: String,
    /// Environment to add, as `(name, value)`. Values that are paths are absolute, because
    /// the process that reads them is several layers below this one and has its own cwd.
    env: Vec<(String, String)>,
}

impl Step {
    fn display(&self) -> String {
        let mut line = String::new();
        if self.cwd != "." {
            line.push_str(&format!("cd {} && ", self.cwd));
        }
        for (name, value) in &self.env {
            line.push_str(&format!("{name}={value} "));
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
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
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
///
/// `triple` is the host target triple, which the sidecar copy's file name has to carry —
/// the bundler resolves `externalBin` by appending it.
pub fn plan(root: &Path, info: &AppInfo, targets: Targets, triple: &str) -> Vec<Step> {
    let mut steps = Vec::new();

    if let Some(bundles) = targets.bundles() {
        // The sidecar, first. `cargo tauri build` builds only the app crate, so nothing else
        // in the plan would produce `cide-hook`, and the bundler's failure when the sidecar is
        // missing names a path with a target triple in it that reads like a cross-compilation
        // problem.
        steps.extend(sidecar_steps(triple));

        let mut env = Vec::new();
        if targets.appimage {
            let runtime = root.join(appimage_runtime_path(triple));
            if !runtime.exists() {
                steps.push(fetch_appimage_runtime(triple));
            }
            env.push((LDAI_RUNTIME_FILE.to_string(), runtime.display().to_string()));
        }

        // Run from the app crate: `cargo tauri build` finds `tauri.conf.json` by walking up
        // from the working directory, and this workspace has no `src-tauri`.
        //
        // `beforeBuildCommand` in that config runs `pnpm build` in `ui/`, so the frontend is
        // not a separate step here — adding one would build it twice.
        steps.push(Step {
            program: "cargo".into(),
            args: vec!["tauri".into(), "build".into(), "--bundles".into(), bundles],
            cwd: APP_CRATE.into(),
            env,
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
            env: Vec::new(),
        });
    }

    steps
}

/// Build `cide-hook` and leave it under the name the Tauri bundler resolves `externalBin` to.
///
/// `install -m755` rather than `cp` so the copy is executable even if the source somehow is
/// not, and because it is one command a reader can paste — the printed plan is meant to be
/// runnable by hand when this task is not.
fn sidecar_steps(triple: &str) -> Vec<Step> {
    vec![
        Step {
            program: "cargo".into(),
            args: vec![
                "build".into(),
                "--release".into(),
                "--locked".into(),
                "-p".into(),
                HOOK_BIN.into(),
            ],
            cwd: ".".into(),
            env: Vec::new(),
        },
        Step {
            program: "install".into(),
            args: vec![
                "-m755".into(),
                format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}"),
                sidecar_path(triple),
            ],
            cwd: ".".into(),
            env: Vec::new(),
        },
    ]
}

/// Where the suffixed copy of `cide-hook` goes, relative to the workspace root.
fn sidecar_path(triple: &str) -> String {
    format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}-{triple}")
}

/// The environment variable `linuxdeploy-plugin-appimage` turns into `--runtime-file`.
const LDAI_RUNTIME_FILE: &str = "LDAI_RUNTIME_FILE";

/// Where the prefetched AppImage type-2 runtime is kept, relative to the workspace root.
const APPIMAGE_RUNTIME_DIR: &str = "target/appimage-runtime";

fn appimage_runtime_path(triple: &str) -> String {
    format!("{APPIMAGE_RUNTIME_DIR}/runtime-{}", runtime_arch(triple))
}

/// The architecture segment of the runtime's file name, which is the triple's first field —
/// the same slice `tauri-bundler` takes for its own tool names.
fn runtime_arch(triple: &str) -> &str {
    triple.split('-').next().unwrap_or(triple)
}

/// Fetch the AppImage type-2 runtime ahead of the build.
///
/// **This step exists because of a hang, not a failure.** The last thing in the chain is
/// `appimagetool`, several layers under `cargo tauri build`, and when it has no runtime file
/// it fetches one from `github.com/AppImage/type2-runtime`. On this machine that download
/// stalled: the socket went to CLOSE-WAIT and the process sat in `futex_do_wait` indefinitely,
/// with `cargo tauri build` holding its pipes and printing nothing after "Bundling
/// cide_0.1.0_amd64.AppImage". There is no timeout anywhere in that stack, so a packaging run
/// simply never returns, and the last line it printed looks like success.
///
/// Fetching it here with `curl --retry`, which does fail rather than hang, and handing it over
/// as `LDAI_RUNTIME_FILE`, takes the network out of the innermost layer. It also makes a
/// second build offline-capable, which the layer below is not.
fn fetch_appimage_runtime(triple: &str) -> Step {
    let arch = runtime_arch(triple);
    Step {
        program: "curl".into(),
        args: vec![
            "-fsSL".into(),
            "--retry".into(),
            "3".into(),
            "--max-time".into(),
            "180".into(),
            "--create-dirs".into(),
            "-o".into(),
            appimage_runtime_path(triple),
            format!(
                "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-{arch}"
            ),
        ],
        cwd: ".".into(),
        env: Vec::new(),
    }
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
        out.push(tauri_cli_check());
        out.push(if info.bundles_the_hook() {
            Verdict::Ok(format!(
                "{TAURI_CONF} ships {HOOK_BIN} as a sidecar (bundle.externalBin)"
            ))
        } else {
            // A failure, not a warning. The bundle would be produced, would install, would
            // launch, and every session in it would run with no hooks — see the module docs.
            Verdict::Fail(format!(
                "{TAURI_CONF} has no `{HOOK_BIN}` in bundle.externalBin, so the package would \
                 ship without it: `cargo tauri build` bundles only the app crate's own \
                 binaries. Sessions would run with no hooks and nothing would report an error"
            ))
        });
        // The `externalBin` path is written relative to the app crate and hardcodes
        // `target/release`. Cargo honours CARGO_TARGET_DIR, the config cannot, and the
        // mismatch surfaces as a missing-sidecar error naming a path that does exist.
        if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
            out.push(Verdict::Warn(format!(
                "CARGO_TARGET_DIR is set to {}; `bundle.externalBin` in {TAURI_CONF} hardcodes \
                 ../../{HOOK_SIDECAR_DIR}, so the sidecar must be copied there by hand",
                dir.to_string_lossy()
            )));
        }
    }

    if targets.appimage {
        out.push(webkit_helper_check());
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

/// The directories `tauri-bundler` searches for WebKit's two helper processes, each joined
/// with `webkit2gtk-4.1`. Copied from `bundle/linux/appimage/linuxdeploy.rs`, whose own
/// comment on the list is `// TODO: Check if it's the same dir name on all systems`.
const WEBKIT_SEARCH_DIRS: [&str; 4] = [
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib64",
    "/usr/lib",
    "/usr/libexec",
];

/// The two out-of-process helpers a WebKitGTK web view cannot render without.
const WEBKIT_HELPERS: [&str; 2] = ["WebKitWebProcess", "WebKitNetworkProcess"];

/// Whether the AppImage will carry WebKit's helper processes.
///
/// **This is the check that catches the quietest failure in this whole file.** The bundler
/// puts `libwebkit2gtk-4.1.so.0` inside the AppImage, and that library spawns two helper
/// executables to do anything at all. It tries to bring them along — it looks for them under
/// `<libdir>/webkit2gtk-4.1/` — and when it does not find them it copies nothing and says
/// nothing: the loop is `if source.exists()`, with no else. On openSUSE they live in
/// `/usr/libexec/libwebkit2gtk-4_1-0/`, which is not a name on that list, so the bundle comes
/// out without them and the run fails on a *different* machine, as a window that never paints.
///
/// A warning rather than a failure because it is not fatal on every host: nothing sets
/// `WEBKIT_EXEC_PATH`, so the bundled library falls back to its compiled-in absolute path, and
/// on a host whose WebKitGTK is laid out like the build machine's that path is there.
fn webkit_helper_check() -> Verdict {
    let bundled: Vec<&str> = WEBKIT_HELPERS
        .into_iter()
        .filter(|helper| {
            WEBKIT_SEARCH_DIRS
                .iter()
                .any(|dir| Path::new(dir).join("webkit2gtk-4.1").join(helper).exists())
        })
        .collect();
    if bundled.len() == WEBKIT_HELPERS.len() {
        return Verdict::Ok("WebKit's helper processes will be bundled".into());
    }
    let elsewhere = find_webkit_helpers_elsewhere();
    let found = match elsewhere {
        Some(dir) => format!(" — this machine keeps them in {dir}"),
        None => String::new(),
    };
    Verdict::Warn(format!(
        "the AppImage will not contain {}: `cargo tauri build` only looks under \
         <libdir>/webkit2gtk-4.1/ and copies nothing when they are absent{found}. Nothing \
         sets WEBKIT_EXEC_PATH either, so the bundled libwebkit2gtk falls back to its \
         compiled-in absolute path and the package only renders on a host that lays \
         WebKitGTK out the way this machine does",
        WEBKIT_HELPERS
            .iter()
            .filter(|h| !bundled.contains(h))
            .copied()
            .collect::<Vec<_>>()
            .join(" and "),
    ))
}

/// Where this machine actually keeps the helpers, if not where the bundler looks.
///
/// Only the directories the bundler already searches are scanned, one level down, which is
/// enough for the `libwebkit2gtk-4_1-0` style of name and cheap enough to run every preflight.
///
/// A machine with both WebKitGTK ABIs installed has two such directories, and naming the 6.0
/// one would send a reader to the GTK 4 build that Tauri 2 does not link. So the 4.1 spelling
/// wins when both are there, and directory order decides nothing.
fn find_webkit_helpers_elsewhere() -> Option<String> {
    let mut fallback = None;
    for dir in WEBKIT_SEARCH_DIRS {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().join(WEBKIT_HELPERS[0]).exists() {
                continue;
            }
            let path = entry.path().display().to_string();
            if path.contains("4_1") || path.contains("4.1") {
                return Some(path);
            }
            fallback.get_or_insert(path);
        }
    }
    fallback
}

/// Whether a `cargo-tauri` that can read a v2 config is installed.
///
/// The presence check this replaces passed on a `cargo-tauri 1.5.12`, which is what a machine
/// that installed `tauri-cli` before Tauri 2 has. That build reads `tauri.conf.json` against
/// the v1 schema and dies on `identifier`/`app`/`bundle` keys it does not know, several
/// screens away from anything that says "wrong version".
fn tauri_cli_check() -> Verdict {
    let install =
        format!("install it with `cargo install tauri-cli --version ^{TAURI_CLI_MAJOR} --locked`");
    let Some(path) = which("cargo-tauri") else {
        return Verdict::Fail(format!("cargo-tauri is not on PATH — {install}"));
    };
    let Ok(output) = Command::new(&path).arg("--version").output() else {
        return Verdict::Warn(format!(
            "cargo-tauri at {} would not report its version",
            path.display()
        ));
    };
    let text = String::from_utf8_lossy(&output.stdout);
    match parse_cli_major(&text) {
        Some(TAURI_CLI_MAJOR) => Verdict::Ok(format!("{} at {}", text.trim(), path.display())),
        Some(major) => Verdict::Fail(format!(
            "cargo-tauri at {} is version {major}.x, but {TAURI_CONF} is a Tauri \
             {TAURI_CLI_MAJOR} config and the workspace links tauri {TAURI_CLI_MAJOR}.x — \
             {install}",
            path.display()
        )),
        None => Verdict::Warn(format!(
            "could not parse a version out of `cargo-tauri --version` ({:?})",
            text.trim()
        )),
    }
}

/// Pull the major version out of `cargo-tauri --version`, which prints `tauri-cli 2.11.4`.
fn parse_cli_major(output: &str) -> Option<u64> {
    output
        .split_whitespace()
        .find_map(|word| word.split('.').next()?.parse::<u64>().ok())
}

/// This machine's target triple, which the sidecar's file name has to carry.
///
/// Asked of `rustc` rather than assembled from `std::env::consts`: those give `x86_64` and
/// `linux` but not the vendor or the libc, and a musl host would get a name the bundler then
/// fails to find.
fn host_triple() -> Result<String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("running `rustc -vV` to learn this machine's target triple")?;
    let text = String::from_utf8_lossy(&output.stdout);
    parse_host_triple(&text)
        .ok_or_else(|| anyhow::anyhow!("`rustc -vV` printed no `host:` line:\n{text}"))
}

fn parse_host_triple(rustc_vv: &str) -> Option<String> {
    rustc_vv
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|triple| triple.trim().to_string())
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
      # `npm install`, not `npm ci`. This repository ships only `ui/pnpm-lock.yaml`, and
      # `npm ci` refuses to run at all without a `package-lock.json`:
      #
      #   npm error The `npm ci` command can only install with an existing package-lock.json
      #
      # — so the build died on its first command. npm rather than pnpm because the GNOME
      # node22 SDK extension ships npm and not pnpm. The cost is that this is the one channel
      # whose transitive dependency versions are not the lockfile's; direct versions are
      # pinned exactly in ui/package.json, so the drift is confined to the tree below them.
      # See docs/adr/0007-packaging-and-distribution.md.
      - npm --prefix ui install --no-audit --no-fund
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
        external_bin: list(conf.pointer("/bundle/externalBin")),
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
            external_bin: vec!["../../target/release/cide-hook".into()],
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
    fn the_real_config_ships_the_hook() {
        // The whole of M11's "and it actually works" rests on this one line of
        // tauri.conf.json. Deleting it produces a bundle that runs and quietly has no hooks,
        // so it is asserted against the checked-in file, not against a fixture.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("tauri.conf.json parses");
        assert!(
            info.bundles_the_hook(),
            "bundle.externalBin must name {HOOK_BIN}: {:?}",
            info.external_bin
        );
    }

    #[test]
    fn the_sidecar_path_matches_the_checked_in_config() {
        // `externalBin` is resolved relative to the crate holding tauri.conf.json; the plan
        // writes its copy relative to the workspace root. The two spellings of one path must
        // meet, and they only do if the config's entry is `../../` plus ours.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("tauri.conf.json parses");
        let entry = info
            .external_bin
            .iter()
            .find(|p| p.ends_with(HOOK_BIN))
            .expect("an externalBin entry for the hook");
        let from_root = root
            .join(APP_CRATE)
            .join(entry)
            .canonicalize()
            .or_else(|_| {
                // The sidecar need not exist yet — the plan builds it — so fall back to
                // comparing the lexical path against the app crate's parent.
                Ok::<_, std::io::Error>(root.join(APP_CRATE).join(entry))
            })
            .unwrap();
        let ours = root.join(format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}"));
        assert!(
            from_root.ends_with(format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}"))
                || from_root == ours.canonicalize().unwrap_or(ours),
            "externalBin resolves to {}, but the plan writes {HOOK_SIDECAR_DIR}/{HOOK_BIN}",
            from_root.display()
        );
    }

    #[test]
    fn a_missing_sidecar_entry_is_a_failure_not_a_warning() {
        let mut info = info();
        info.external_bin.clear();
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::ALL);
        assert!(
            checks
                .iter()
                .any(|c| matches!(c, Verdict::Fail(d) if d.contains(HOOK_BIN))),
            "{checks:?}"
        );
    }

    #[test]
    fn a_sidecar_in_another_directory_still_counts() {
        // The check is on the file name: moving the sidecar must not silently disarm it.
        let mut info = info();
        info.external_bin = vec!["sidecars/cide-hook".into()];
        assert!(info.bundles_the_hook());
        info.external_bin = vec!["sidecars/cide-hooked".into()];
        assert!(!info.bundles_the_hook());
    }

    #[test]
    fn the_hook_is_built_and_suffixed_before_the_bundler_runs() {
        // Order is the point: `cargo tauri build` builds only the app crate, and it resolves
        // externalBin by appending the triple, so both of these must already have happened.
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            Targets::ALL,
            "x86_64-unknown-linux-gnu",
        );
        let build = steps
            .iter()
            .position(|s| s.program == "cargo" && s.args.contains(&HOOK_BIN.to_string()))
            .expect("a step that builds the hook");
        let copy = steps
            .iter()
            .position(|s| s.program == "install")
            .expect("a step that names the sidecar copy");
        let bundle = steps
            .iter()
            .position(|s| s.args.first().map(String::as_str) == Some("tauri"))
            .expect("the bundler step");
        assert!(build < copy && copy < bundle, "{steps:?}");
        assert!(
            steps[copy]
                .args
                .last()
                .is_some_and(|a| a.ends_with("cide-hook-x86_64-unknown-linux-gnu")),
            "the copy must carry the target triple: {:?}",
            steps[copy]
        );
    }

    #[test]
    fn asking_for_no_tauri_target_does_not_build_the_hook() {
        let targets = Targets {
            appimage: false,
            deb: false,
            flatpak: true,
        };
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            targets,
            "x86_64-unknown-linux-gnu",
        );
        assert!(steps.iter().all(|s| s.program != "install"), "{steps:?}");
    }

    #[test]
    fn the_appimage_runtime_is_prefetched_and_handed_to_the_bundler() {
        // Without LDAI_RUNTIME_FILE, appimagetool fetches the runtime itself, six layers down,
        // and a stalled download there hangs the whole build with no timeout and no output.
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            Targets::ALL,
            "x86_64-unknown-linux-gnu",
        );
        let fetch = steps
            .iter()
            .position(|s| s.program == "curl")
            .expect("a step that fetches the runtime");
        let bundle = steps
            .iter()
            .position(|s| s.args.first().map(String::as_str) == Some("tauri"))
            .expect("the bundler step");
        assert!(fetch < bundle, "{steps:?}");
        assert!(
            steps[fetch]
                .args
                .last()
                .is_some_and(|u| u.ends_with("runtime-x86_64")),
            "{:?}",
            steps[fetch]
        );
        let handed = &steps[bundle].env;
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0].0, LDAI_RUNTIME_FILE);
        assert!(
            handed[0].1.starts_with('/'),
            "appimagetool runs with its own cwd, so this has to be absolute: {handed:?}"
        );
    }

    #[test]
    fn an_already_fetched_runtime_is_not_fetched_again() {
        let dir = std::env::temp_dir().join(format!("cide-xtask-{}", std::process::id()));
        let runtime = dir.join(appimage_runtime_path("x86_64-unknown-linux-gnu"));
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::write(&runtime, b"not really a runtime").unwrap();

        let steps = plan(&dir, &info(), Targets::ALL, "x86_64-unknown-linux-gnu");
        assert!(steps.iter().all(|s| s.program != "curl"), "{steps:?}");
        // …but it is still handed over, or the bundler downloads its own.
        assert!(
            steps
                .iter()
                .any(|s| s.env.iter().any(|(k, _)| k == LDAI_RUNTIME_FILE)),
            "{steps:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn asking_for_deb_alone_needs_no_appimage_runtime() {
        let targets = Targets {
            appimage: false,
            deb: true,
            flatpak: false,
        };
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            targets,
            "x86_64-unknown-linux-gnu",
        );
        assert!(steps.iter().all(|s| s.program != "curl"), "{steps:?}");
        assert!(steps.iter().all(|s| s.env.is_empty()), "{steps:?}");
    }

    #[test]
    fn the_webkit_helper_check_agrees_with_this_machine() {
        // Not a fixture: the point of the check is to describe the machine it runs on, and a
        // mocked filesystem would only assert that the mock was read. It has to be one or the
        // other, and either way it must name the helpers.
        let verdict = webkit_helper_check();
        assert!(
            matches!(verdict, Verdict::Ok(_) | Verdict::Warn(_)),
            "never a hard failure: {verdict:?}"
        );
        let ok = WEBKIT_SEARCH_DIRS.iter().any(|d| {
            Path::new(d)
                .join("webkit2gtk-4.1/WebKitWebProcess")
                .exists()
        });
        assert_eq!(matches!(verdict, Verdict::Ok(_)), ok, "{verdict:?}");
        if let Verdict::Warn(detail) = &verdict {
            assert!(detail.contains("WebKitWebProcess"), "{detail}");
        }
    }

    #[test]
    fn the_helper_search_prefers_the_abi_tauri_links() {
        // On a machine carrying both WebKitGTK ABIs, `/usr/libexec` holds
        // `libwebkit2gtk-4_1-0` and `libwebkitgtk-6_0-0`, and directory order decides which is
        // seen first. Naming the 6.0 one points a reader at the GTK 4 build Tauri 2 does not
        // link. Only assertable where both exist, so it is a conditional rather than a fixture
        // — the alternative was a temp tree, which would only prove the temp tree was read.
        let Some(found) = find_webkit_helpers_elsewhere() else {
            return;
        };
        let has_both = Path::new("/usr/libexec/libwebkit2gtk-4_1-0").exists()
            && Path::new("/usr/libexec/libwebkitgtk-6_0-0").exists();
        if has_both {
            assert!(found.contains("4_1"), "picked the wrong ABI: {found}");
        }
    }

    #[test]
    fn the_runtime_is_named_after_the_triples_architecture() {
        assert_eq!(runtime_arch("x86_64-unknown-linux-gnu"), "x86_64");
        assert_eq!(runtime_arch("aarch64-unknown-linux-gnu"), "aarch64");
    }

    #[test]
    fn a_step_with_environment_is_still_pasteable() {
        let step = Step {
            program: "cargo".into(),
            args: vec!["tauri".into(), "build".into()],
            cwd: APP_CRATE.into(),
            env: vec![("LDAI_RUNTIME_FILE".into(), "/tmp/runtime-x86_64".into())],
        };
        assert_eq!(
            step.display(),
            "cd crates/cide-app && LDAI_RUNTIME_FILE=/tmp/runtime-x86_64 cargo tauri build"
        );
    }

    #[test]
    fn a_v1_cli_is_not_good_enough() {
        // The check this replaced only asked whether cargo-tauri existed, and a `cargo-tauri
        // 1.5.12` — what a machine that installed tauri-cli before Tauri 2 has — passed it.
        assert_eq!(parse_cli_major("tauri-cli 1.5.12\n"), Some(1));
        assert_eq!(parse_cli_major("tauri-cli 2.11.4\n"), Some(2));
        assert_eq!(parse_cli_major("cargo-tauri 2.0.0-rc.3"), Some(2));
        assert_eq!(parse_cli_major("no version here"), None);
    }

    #[test]
    fn the_host_triple_comes_from_rustc() {
        assert_eq!(
            parse_host_triple("rustc 1.92.0\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\n")
                .as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(parse_host_triple("rustc 1.92.0\n"), None);
        // Real output, so a change in rustc's formatting fails here rather than producing a
        // sidecar named after a triple nothing looks for.
        assert!(host_triple().expect("rustc -vV").contains('-'));
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
    fn the_manifest_never_uses_npm_ci() {
        // `npm ci` cannot run here: the repository has a pnpm lockfile and no
        // `package-lock.json`, and npm exits with EUSAGE rather than falling back. It is the
        // manifest's first build command, so the whole channel failed on it, and nothing in
        // this repository would have noticed — flatpak-builder is the only thing that reads
        // this file.
        let manifest = flatpak_manifest(&info());
        assert!(
            !manifest.contains("npm --prefix ui ci"),
            "npm ci needs a package-lock.json this repository does not have"
        );
        assert!(manifest.contains("npm --prefix ui install"));
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
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            Targets::ALL,
            "x86_64-unknown-linux-gnu",
        );
        let tauri: Vec<&Step> = steps
            .iter()
            .filter(|s| s.args.first().map(String::as_str) == Some("tauri"))
            .collect();
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
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            targets,
            "x86_64-unknown-linux-gnu",
        );
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
            env: Vec::new(),
        };
        assert_eq!(step.display(), "cd crates/cide-app && cargo tauri build");
    }
}
