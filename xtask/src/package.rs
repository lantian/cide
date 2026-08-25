//! Packaging: AppImage, .deb and a Flatpak manifest on Linux; `.app` and `.dmg` on macOS.
//!
//! # Nothing here cross-compiles, and the task says so rather than printing a plan that cannot run
//!
//! A bundle is built by and for the host. Linking `cide-app` for `*-apple-darwin` needs Apple's
//! linker and the macOS SDK, whose licence restricts it to Apple hardware, and the bundlers
//! themselves shell out to host tools — `linuxdeploy` and `dpkg-deb` on one side, `codesign`,
//! `hdiutil` and `sips` on the other. Tauri documents cross-compiling Windows from Linux
//! (`cargo-xwin`) and nothing else. So `--dmg` on a Linux host is a `Verdict::Fail` with the
//! reason in it, not a plan that fails twenty minutes later inside a tool nobody has: naming a
//! target the host cannot produce is a mistake worth catching in the first second.
//!
//! Naming no target at all means *everything this host can build*, which is why
//! [`Targets::for_host`] takes a triple. On Linux that is the three below, unchanged.
//!
//! # The macOS half is configuration, not experience
//!
//! **cide has never been compiled, run or bundled on macOS.** What exists is
//! `crates/cide-app/tauri.macos.conf.json`, the plan, and the preflight verdicts — all of which
//! were written by reading `tauri-utils`' config schema and `tauri-bundler`'s macOS bundler
//! rather than by watching one succeed. `README.md`'s Platforms section is the honest record;
//! keep the two in step.
//!
//! That overlay file is safe in a way `tauri.linux.conf.json` is not, and the asymmetry is the
//! whole reason one exists and the other is refused a few lines below. `tauri-build` reads the
//! platform overlay for the **host** on every cargo invocation, so a Linux overlay carrying a
//! bundling-only key breaks `cargo build --workspace` on the only platform this repository is
//! developed on. A macOS overlay is never read here at all — and on a Mac it would be read by
//! every `cargo build` too, which is why it must carry no `externalBin` (`the_macos_overlay…`
//! test, and a preflight verdict, both say so).
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
//! So the hook rides along as a Tauri *sidecar*: `bundle.externalBin` names
//! `../../target/release/cide-hook`, the bundler looks for that path with the target triple
//! appended, and `Settings::copy_binaries` strips the triple back off when it copies it into
//! `usr/bin/` — beside `cide`, which is exactly where `cmd::session::hook_settings` looks
//! (`current_exe().parent().join("cide-hook")`). Two steps in the plan produce that suffixed
//! copy. The alternative — `bundle.linux.appimage.files` — was rejected because it is
//! AppImage-only, so the `.deb` would have silently kept shipping without the hook.
//!
//! # …but that key may not live in `tauri.conf.json`, or nothing in this repository builds
//!
//! `bundle.externalBin` is read by `tauri-build`, not only by the bundler: the `cide-app`
//! build script copies every named sidecar into the target directory on **every** cargo
//! invocation, and errors with `resource path ... doesn't exist` when one is missing. Putting
//! the key in `crates/cide-app/tauri.conf.json` therefore makes a plain `cargo build`,
//! `cargo test --workspace` and `cargo clippy --workspace` fail on any tree that has not
//! already produced a *release* `target/release/cide-hook-<triple>` — a fresh clone, and every
//! CI run, because `.github/workflows/ci.yml` builds the workspace before anything packages
//! anything. The failure looks like a broken checkout, not like a packaging decision.
//!
//! So the key lives in `crates/cide-app/tauri.bundle.conf.json`, which nothing reads unless
//! it is asked to, and the plan asks: `cargo tauri build --config tauri.bundle.conf.json`.
//! `tauri-cli` merges that patch over `tauri.conf.json` for its own bundling *and* exports the
//! merged patch as `TAURI_CONFIG`, which `tauri-build` picks up — so the sidecar reaches both
//! halves of the build, and only during a packaging run. `preflight` fails if the key ever
//! comes back to the base config; `the_base_config_must_not_carry_the_sidecar` is the test.
//!
//! Rejected: `tauri.linux.conf.json`, the platform-overlay file. `tauri-build` reads that one
//! too (`tauri_utils::config::parse::read_platform`), so it breaks the workspace build in
//! exactly the same way on the only platform this file targets.
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
use sha2::{Digest, Sha256};

/// The Tauri configuration, which is where the identifier, version and bundle targets live.
const TAURI_CONF: &str = "crates/cide-app/tauri.conf.json";

/// The bundle-only overlay, merged over `TAURI_CONF` by `cargo tauri build --config`.
///
/// Everything here is a key that would break a plain `cargo build` if it sat in the base
/// config — today that is `bundle.externalBin`. See the module docs.
const TAURI_BUNDLE_CONF: &str = "crates/cide-app/tauri.bundle.conf.json";

/// The same file as `TAURI_BUNDLE_CONF`, spelled the way `cargo tauri build` will see it.
///
/// `--config` resolves a path against the *process* working directory, and the bundler step
/// runs in `APP_CRATE`. Deriving it rather than writing it twice, so a move of the file
/// cannot leave the flag pointing at the old name.
fn bundle_conf_arg() -> String {
    Path::new(TAURI_BUNDLE_CONF)
        .strip_prefix(APP_CRATE)
        .unwrap_or(Path::new(TAURI_BUNDLE_CONF))
        .display()
        .to_string()
}

/// Where the generated Flatpak files are checked in.
const FLATPAK_DIR: &str = "packaging/flatpak";

/// Where the source tarball is written, relative to the workspace root.
///
/// Under `target/release/bundle/` beside the compiled artefacts, and not at the root of
/// `target/`, because that directory is what `artefacts` enumerates and what the completion
/// line points a reader at. A release script globs one directory; a tarball anywhere else is
/// the asset somebody forgets to upload.
const SRC_BUNDLE_DIR: &str = "target/release/bundle/src";

/// The tarball's file name: `cide-0.1.0-src.tar.gz`.
///
/// It has to carry the version, because a release asset is downloaded away from the page that
/// explains it, and it has to be distinguishable from GitHub's own auto-attached "Source code
/// (tar.gz)" — which is named after the *tag* and is a different file (no prefix control, and
/// nothing guarantees what a future GitHub puts in it). `-src` after the version rather than
/// before it so this sorts next to `cide_0.1.0_amd64.AppImage` and reads as a variant of the
/// same product.
fn src_archive_name(info: &AppInfo) -> String {
    format!("{}-{}-src.tar.gz", info.product_name, info.version)
}

/// The directory the tarball unpacks into: `cide-0.1.0/`.
///
/// Never empty. `git archive` with no prefix writes 889 entries at the top level, so unpacking
/// it in a downloads directory scatters the whole repository across it. The trailing slash is
/// required — `--prefix` is a string prepended to every path, not a directory name.
fn src_prefix(info: &AppInfo) -> String {
    format!("{}-{}/", info.product_name, info.version)
}

/// The tarball's path, relative to the workspace root.
fn src_archive_path(info: &AppInfo) -> String {
    format!("{SRC_BUNDLE_DIR}/{}", src_archive_name(info))
}

/// Where the binary tarball is written, relative to the workspace root.
///
/// Its own subdirectory under `target/release/bundle/`, for the reason [`SRC_BUNDLE_DIR`] gives:
/// `artefacts` enumerates that tree by subdirectory name, and an artefact outside it is the
/// asset a release forgets to upload.
const TARBALL_BUNDLE_DIR: &str = "target/release/bundle/tarball";

/// Where the tarball's contents are assembled before `tar` reads them.
///
/// Under `target/` and not a temporary directory: the plan is printed before it is run and is
/// meant to be pasteable, and a `$TMPDIR/tmp.XXXXXX` in a printed command is a path the reader
/// cannot reproduce. It also survives the run, so a failed archive step can be inspected.
const TARBALL_STAGE_DIR: &str = "target/release/tarball-stage";

/// The icon sizes the tarball installs, which are the ones `crates/cide-app/icons/` holds at
/// `<n>x<n>.png` — the same set the `.deb` ships into `share/icons/hicolor`.
///
/// `icon.png` and `icon.svg` are deliberately absent: the first is the bundler's source image
/// and has no `hicolor` size directory to live in, and nothing here renders SVG.
const TARBALL_ICON_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

/// The tarball's file name: `cide-0.1.0-linux-x86_64.tar.gz`.
///
/// `linux-<arch>` and not the bundler's `_<version>_amd64` spelling, because this file is not
/// produced by the bundler and pretending otherwise would suggest the two are interchangeable.
/// `x86_64` is the triple's own name for the architecture — the same slice
/// [`runtime_arch`] takes — and it is what `uname -m` answers on the machine that will unpack
/// it, which `amd64` is not.
fn tarball_name(info: &AppInfo, triple: &str) -> String {
    format!(
        "{}-{}-linux-{}.tar.gz",
        info.product_name,
        info.version,
        runtime_arch(triple)
    )
}

/// The tarball's path, relative to the workspace root.
fn tarball_path(info: &AppInfo, triple: &str) -> String {
    format!("{TARBALL_BUNDLE_DIR}/{}", tarball_name(info, triple))
}

/// The directory the tarball unpacks into: `cide-0.1.0/`.
///
/// No trailing slash, unlike [`src_prefix`] — that one is `git archive --prefix`, which is
/// string concatenation, while this one is a real directory under the staging root and is
/// passed to `tar` as a path.
fn tarball_prefix(info: &AppInfo) -> String {
    format!("{}-{}", info.product_name, info.version)
}

/// The directory `cargo tauri build` must run from — the crate holding `tauri.conf.json`.
const APP_CRATE: &str = "crates/cide-app";

/// The second binary. See the module docs: without it in the bundle the product runs with
/// no hooks and nothing reports an error.
const HOOK_BIN: &str = "cide-hook";

/// The first binary — `crates/cide-app/Cargo.toml`'s `[[bin]] name`, which is what the release
/// profile writes into `target/release/`.
///
/// Spelled out rather than taken from `info.product_name`, which happens to be the same string
/// today and is not the same *fact*: `productName` is the name a user sees on a window and a
/// desktop entry, and nothing stops it becoming "Cide" or "cide IDE" without the binary moving.
/// `the_app_binary_is_what_the_crate_declares` asserts the two agree with the crate.
const APP_BIN: &str = "cide";

/// The package `cargo build -p` is given for [`APP_BIN`]. Not the same string: the crate is
/// `cide-app` and the binary it produces is `cide`.
const APP_PACKAGE: &str = "cide-app";

/// Where the sidecar copy is written, relative to the workspace root. `tauri.conf.json`'s
/// `externalBin` entry names the same path relative to `crates/cide-app`, and the two have to
/// agree; `the_sidecar_path_matches_the_checked_in_config` asserts they do.
const HOOK_SIDECAR_DIR: &str = "target/release";

/// The third binary: cide's own build of rust-analyzer. (M25)
///
/// Named `cide-rust-analyzer` on disk, never `rust-analyzer`, because the tarball's contract
/// is "put `bin/` on your PATH" — a file there called `rust-analyzer` would shadow the user's
/// own for every shell on the machine. `cide_lsp::discover`'s bundled table maps the registry
/// name to this file name; the two crates meet only on this string.
const RA_BIN: &str = "cide-rust-analyzer";

/// Where the fork is checked out, relative to the workspace root: under `../forks/`, beside
/// the salsa fork its manifest patches to (`../salsa` *from the fork*, so the two must stay
/// siblings of each other wherever the pair lives). **Not** a workspace member — 400k lines
/// of rust-analyzer joining every `cargo build --workspace` was the option that lost — so
/// nothing in CI needs it; only a packaging run does, and preflight is where its absence is
/// reported.
const RA_FORK_DIR: &str = "../forks/rust-analyzer";

/// The pin: which fork revision a package carries, checked into **this** repository.
///
/// Shell-sourceable `KEY=VALUE` lines (`CIDE_RA_URL`, `CIDE_RA_REV`) so release.yml can
/// `. packaging/rust-analyzer.lock` while this module parses the same bytes — one pin, three
/// consumers (preflight, the generated flatpak manifest, the release workflow), zero copies.
const RA_LOCK: &str = "packaging/rust-analyzer.lock";

/// The fourth binary: cide's own build of gopls. (M26)
///
/// Same two-name rule as [`RA_BIN`], same reason: `bin/` on PATH must never shadow the
/// user's own `gopls`.
const GOPLS_BIN: &str = "cide-gopls";

/// Where the gopls fork is checked out: the golang/tools repository (gopls is its `gopls/`
/// module), branch `cide`, beside the other forks. Like [`RA_FORK_DIR`], never a workspace
/// member, needed only by packaging runs.
const GOPLS_FORK_DIR: &str = "../forks/tools";

/// The gopls pin, [`RA_LOCK`]'s twin: `CIDE_GOPLS_URL` / `CIDE_GOPLS_REV`, shell-sourceable.
const GOPLS_LOCK: &str = "packaging/gopls.lock";

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

/// The macOS-only overlay, read by `tauri-build` and `cargo tauri build` when the host is a Mac
/// and by nothing at all here. See the module docs for why this one is safe and
/// `tauri.linux.conf.json` is not.
const TAURI_MACOS_CONF: &str = "crates/cide-app/tauri.macos.conf.json";

/// Which artefacts to produce.
///
/// A flag each rather than a `Vec<String>` so that the host check below can ask "did you name
/// anything this machine cannot build" as a compile-checked question rather than by matching
/// strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub appimage: bool,
    pub deb: bool,
    pub flatpak: bool,
    /// macOS: the `.app` bundle. The `.dmg` is built from it, so asking for a dmg alone still
    /// produces one — `cargo tauri build --bundles dmg` runs the app bundler first.
    pub app: bool,
    /// macOS: the disk image, which is what a person downloads.
    pub dmg: bool,
    /// The Linux binary tarball: the release binaries, the desktop entry and the icons under
    /// one prefix directory, gzipped.
    ///
    /// The artefact for somebody who wants the program without an AppImage's runtime, a package
    /// manager, or root. Two things it is *not*, both of which a reader will assume from its
    /// neighbours here:
    ///
    /// * **Not reproducible.** `--src` is `git archive` and its bytes follow from the commit, so
    ///   a published sha256 is checkable by anyone. This one contains `rustc` output; nothing
    ///   about it is byte-stable across two machines, and the sha256 the run prints describes
    ///   that build alone.
    /// * **Not self-contained.** The AppImage carries WebKitGTK; this carries a dynamically
    ///   linked `cide` that needs the host's WebKitGTK 4.1 already installed. That is the trade
    ///   it exists to offer — roughly a tenth of the size, and a dependency on the distribution.
    pub tarball: bool,
    /// The source tarball for a release page: `git archive` of `HEAD`, nothing compiled.
    ///
    /// The one target that is not platform-bound. Every other field names a bundle that only
    /// one kind of host can produce; this one is `git` and a gzip, so `impossible_on` never
    /// names it and `--src` is honoured everywhere. What *is* platform-bound is who produces it
    /// by default — see [`Targets::MACOS`].
    pub src: bool,
    /// Whether the source tarball was **named on the command line** rather than inherited from
    /// the host's default set.
    ///
    /// It changes exactly one verdict: a dirty working tree. Asked for explicitly, that is a
    /// `Fail` — the caller is cutting a release artifact, and one built from a tree that does not
    /// match HEAD is a claim nobody can reproduce from the tag. Inherited from the default set,
    /// the same tree is a `Warn` and the archive step is dropped, because the caller typed
    /// `package --run` to build an AppImage of their work in progress and refusing to build
    /// anything at all, over a tarball they never mentioned, answers a question they did not ask.
    /// The advice differs too: "commit or stash" is right for a release and wrong for someone
    /// deliberately testing uncommitted work.
    pub src_named: bool,
}

impl Targets {
    /// Everything Linux is responsible for producing.
    pub const LINUX: Self = Self {
        appimage: true,
        deb: true,
        flatpak: true,
        app: false,
        dmg: false,
        tarball: true,
        src: true,
        // Inherited, not asked for. A dirty tree therefore warns and drops the archive step
        // rather than failing the whole run — see the field's own docs.
        src_named: false,
    };

    /// Everything macOS is responsible for producing.
    ///
    /// `src` is deliberately false here and true in [`Targets::LINUX`], and it is the one place
    /// this struct stops meaning "what the host *can* build". A Mac runs `git archive` perfectly
    /// well — `--src` on a Mac is honoured, and `impossible_on` never names it. But the source
    /// tarball is not a per-platform artefact: a release matrix that produced it on both hosts
    /// would upload two files with the same name whose bytes differ in the gzip layer alone (the
    /// tar layers are identical, and `git get-tar-commit-id` reads the same commit out of both),
    /// and whichever upload lost would leave a published checksum matching neither. One artefact,
    /// one producer, and the producer is the platform every gate in this repository runs on.
    pub const MACOS: Self = Self {
        appimage: false,
        deb: false,
        flatpak: false,
        app: true,
        dmg: true,
        tarball: false,
        src: false,
        src_named: false,
    };

    /// Everything the host named by `triple` is responsible for producing, which is what naming
    /// no target means.
    ///
    /// "Responsible for" and not "capable of", and the difference is exactly one field: `src`.
    /// See [`Targets::MACOS`] for why the source tarball has one producer rather than two.
    /// Everything else here is the stronger statement — a target absent from a host's set is one
    /// that host cannot build at all, which is what [`Targets::impossible_on`] refuses.
    ///
    /// Taking a triple rather than reading `cfg!(target_os)` is what makes both branches
    /// testable from one machine — the same reason `cide_core::keymap::platform_layer` takes a
    /// bool. It is also the honest question: the artefacts are decided by the machine running
    /// the bundler, and `rustc -vV` is where that machine names itself.
    pub fn for_host(triple: &str) -> Self {
        if is_macos_triple(triple) {
            Self::MACOS
        } else {
            Self::LINUX
        }
    }

    /// The targets named here that `triple` cannot build.
    ///
    /// Returned as names rather than as a bool so the failure can say which ones, which is the
    /// difference between a verdict a reader can act on and one they have to guess at.
    ///
    /// `src` has no row on purpose: no host is unable to produce it, so `--src` on a Mac is a
    /// plan and not a refusal. It is absent from `Targets::MACOS` for a different reason
    /// entirely — one artefact, one producer — and conflating the two would turn a defaults
    /// decision into a refusal nobody could override.
    fn impossible_on(self, triple: &str) -> Vec<&'static str> {
        let macos_host = is_macos_triple(triple);
        [
            (self.appimage, "appimage", false),
            (self.deb, "deb", false),
            (self.flatpak, "flatpak", false),
            (self.tarball, "tarball", false),
            (self.app, "app", true),
            (self.dmg, "dmg", true),
        ]
        .into_iter()
        .filter(|(wanted, _, needs_macos)| *wanted && *needs_macos != macos_host)
        .map(|(_, name, _)| name)
        .collect()
    }

    /// The `--bundles` value for `cargo tauri build`, or `None` when no Tauri target was
    /// asked for. (Flatpak is not one: it is built by `flatpak-builder` from a manifest. Nor is
    /// `src`: it is `git archive`, and it compiles nothing at all. Nor is `tarball`: the
    /// bundler has no format for it, so it is `install` and `tar` over what the release
    /// profile already produced.)
    fn bundles(self) -> Option<String> {
        let mut names = Vec::new();
        if self.appimage {
            names.push("appimage");
        }
        if self.deb {
            names.push("deb");
        }
        if self.app {
            names.push("app");
        }
        if self.dmg {
            names.push("dmg");
        }
        (!names.is_empty()).then(|| names.join(","))
    }
}

/// Whether a target triple names macOS.
///
/// `-apple-darwin` and not `apple` alone: `aarch64-apple-ios` is a real triple and is not a host
/// this task can build for either.
fn is_macos_triple(triple: &str) -> bool {
    triple.contains("-apple-darwin")
}

/// What the task was asked to do.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// `None` means "everything this host can build" — see [`Targets::for_host`]. Kept as an
    /// option rather than resolved in `main.rs` because the host is learned from `rustc -vV`,
    /// which is this module's business and not the CLI parser's.
    pub targets: Option<Targets>,
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
    /// `bundle.externalBin` from `TAURI_BUNDLE_CONF` — the sidecars the bundler copies beside
    /// the main binary.
    pub external_bin: Vec<String>,
    /// `bundle.externalBin` from `TAURI_CONF`. Must stay empty: `tauri-build` acts on it on
    /// every cargo invocation, so anything here breaks `cargo build --workspace`.
    pub base_external_bin: Vec<String>,
    /// `bundle.linux.appimage.files` from `TAURI_BUNDLE_CONF`, as AppDir path -> source path.
    ///
    /// AppImage-only on purpose. A `.deb` depends on the distribution's own `webkit2gtk`
    /// package and must not carry a second copy of its helper processes.
    pub appimage_files: Vec<(String, String)>,
    /// `bundle.targets` from `TAURI_MACOS_CONF`, the macOS platform overlay.
    ///
    /// Separate from `bundle_targets` because the overlay *replaces* the base array rather than
    /// extending it (the merge is RFC 7386, in which an array is a scalar) — so on a Mac the
    /// base config's `["appimage", "deb"]` is not what the bundler sees, and a preflight that
    /// checked the base array would pass while producing nothing.
    pub macos_bundle_targets: Vec<String>,
    /// `bundle.icon` from `TAURI_MACOS_CONF`, for the same reason.
    pub macos_icons: Vec<String>,
    /// `bundle.externalBin` from `TAURI_MACOS_CONF`. Must stay empty, exactly like
    /// `base_external_bin`: on a Mac, `tauri-build` reads this overlay on every cargo
    /// invocation, so a sidecar key here would break `cargo build --workspace` there while
    /// leaving Linux green — a red build on the one platform nobody here can reproduce.
    pub macos_external_bin: Vec<String>,
    /// `build.beforeBuildCommand` from `TAURI_CONF` — the frontend build `cargo tauri build`
    /// runs before it compiles anything. `None` when the config configures none.
    pub before_build: Option<BeforeBuild>,
}

/// `build.beforeBuildCommand`: the shell line, and the directory it runs in.
///
/// Its own type rather than two `Option<String>` fields on [`AppInfo`], because the pair is what
/// the preflight has a question about — *which program does this line need, and does this machine
/// have it* — and a `cwd` without a script means nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeforeBuild {
    /// The whole line, exactly as `cargo tauri build` hands it to `sh -c`
    /// (`tauri-cli-2.11.4` `src/helpers/mod.rs:105`; `cmd /S /C` on Windows, which is not a
    /// platform here).
    pub script: String,
    /// The config's own `cwd` string, which `tauri-cli` passes straight to
    /// `Command::current_dir` — so it is relative to the *process* working directory, and the
    /// bundler step in [`plan`] runs from [`APP_CRATE`]. `None` when the config gives none, in
    /// which case tauri uses its own frontend directory instead.
    pub cwd: Option<String>,
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

    /// Whether `bundle.externalBin` carries the rust-analyzer fork. Same matching rule as
    /// [`Self::bundles_the_hook`], and the same stakes: a bundle without it installs, runs,
    /// and silently falls back to the user's PATH rust-analyzer — which works, so nobody
    /// notices the feature the package exists to ship is not in it.
    fn bundles_the_fork(&self) -> bool {
        self.external_bin
            .iter()
            .any(|p| Path::new(p).file_name().is_some_and(|n| n == RA_BIN))
    }

    /// Whether `bundle.externalBin` carries the gopls build. Same rule, same silent-fallback
    /// stakes as [`Self::bundles_the_fork`].
    fn bundles_the_gopls(&self) -> bool {
        self.external_bin
            .iter()
            .any(|p| Path::new(p).file_name().is_some_and(|n| n == GOPLS_BIN))
    }
}

/// A fork pin — the shape both [`RA_LOCK`] and [`GOPLS_LOCK`] state (each with its own key
/// names, one parser).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaLock {
    pub url: String,
    pub rev: String,
}

/// Parse [`RA_LOCK`]'s bytes. Pure, so the format has a test.
///
/// `KEY=VALUE`, one per line, `#` comments and blank lines ignored — the subset of shell that
/// is also trivially a config format, because release.yml sources the same file. Unknown keys
/// are ignored rather than refused: the file may grow (a `CIDE_RA_BRANCH` hint, say) and an
/// old xtask refusing a new lock would couple the two directions of update for no safety.
fn parse_ra_lock(text: &str) -> Option<RaLock> {
    parse_lock(text, "CIDE_RA_URL", "CIDE_RA_REV")
}

/// [`GOPLS_LOCK`]'s parser — same format, its own key names, so sourcing both locks in one
/// shell (release.yml does) cannot have one clobber the other.
fn parse_gopls_lock(text: &str) -> Option<RaLock> {
    parse_lock(text, "CIDE_GOPLS_URL", "CIDE_GOPLS_REV")
}

fn parse_lock(text: &str, url_key: &str, rev_key: &str) -> Option<RaLock> {
    let mut url = None;
    let mut rev = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key == url_key {
                url = Some(value.trim().to_string());
            } else if key == rev_key {
                rev = Some(value.trim().to_string());
            }
        }
    }
    Some(RaLock {
        url: url.filter(|u| !u.is_empty())?,
        rev: rev.filter(|r| !r.is_empty())?,
    })
}

/// Read and parse [`RA_LOCK`], or say what is wrong with it in a sentence.
pub fn read_ra_lock(root: &Path) -> Result<RaLock> {
    let path = root.join(RA_LOCK);
    let text = fs::read_to_string(&path).context(format!("reading {}", path.display()))?;
    parse_ra_lock(&text).ok_or_else(|| {
        anyhow::anyhow!(
            "{} does not state both CIDE_RA_URL and CIDE_RA_REV",
            path.display()
        )
    })
}

/// Read and parse [`GOPLS_LOCK`], or say what is wrong with it in a sentence.
pub fn read_gopls_lock(root: &Path) -> Result<RaLock> {
    let path = root.join(GOPLS_LOCK);
    let text = fs::read_to_string(&path).context(format!("reading {}", path.display()))?;
    parse_gopls_lock(&text).ok_or_else(|| {
        anyhow::anyhow!(
            "{} does not state both CIDE_GOPLS_URL and CIDE_GOPLS_REV",
            path.display()
        )
    })
}

/// Every icon file the requested targets could need, each with the config that names it.
///
/// Free function rather than a method because it is about a *request*, not about the repository.
fn icon_list(info: &AppInfo, targets: Targets) -> Vec<(&str, &'static str)> {
    let mut out: Vec<(&str, &'static str)> = Vec::new();
    // `tarball` joins them: it installs a subset of the same `bundle.icon` files into
    // `share/icons/hicolor`, so the same existence check is the one it needs.
    if targets.appimage || targets.deb || targets.flatpak || targets.tarball {
        out.extend(info.icons.iter().map(|i| (i.as_str(), TAURI_CONF)));
    }
    if targets.app || targets.dmg {
        for icon in &info.macos_icons {
            if !out.iter().any(|(seen, _)| *seen == icon.as_str()) {
                out.push((icon.as_str(), TAURI_MACOS_CONF));
            }
        }
    }
    out
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
    let targets = opts.targets.unwrap_or_else(|| Targets::for_host(&triple));
    let checks = preflight(root, &info, targets, &triple);
    println!(
        "preflight for {} {} on {triple}",
        info.product_name, info.version
    );
    for check in &checks {
        println!("  [{}] {}", check.marker(), check.detail());
    }
    let failures: Vec<&Verdict> = checks
        .iter()
        .filter(|c| matches!(c, Verdict::Fail(_)))
        .collect();

    let steps = plan(root, &info, targets, &triple);
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
        // The checksum of the file that will actually be uploaded, printed where a release
        // script can copy it. It matters most for the source tarball, whose whole promise is
        // that anyone who checks out the tag can reproduce these bytes — but a release page
        // wants one per asset, and computing it here means the number quoted in the notes came
        // from the artefact rather than from a second command somebody might run on a stale
        // file. Silent for the `.app`, which is a directory.
        if let Some(sum) = sha256_file(&root.join(&path)) {
            println!("      sha256  {sum}");
        }
    }
    Ok(())
}

/// The sha256 of a file, or `None` when it is not a readable regular file.
fn sha256_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

/// The bundles that exist under `target/release/bundle`, with their sizes.
///
/// Printed after a build because "it succeeded" is not the interesting part — a bundle that
/// came out at 12 MiB has not picked up WebKit's helper processes, and the number is the
/// cheapest way to notice.
fn artefacts(root: &Path) -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let bundle = root.join("target/release/bundle");
    // `macos` is where tauri-bundler puts the `.app`, `dmg` where it puts the disk image, `src`
    // where the plan above puts the source tarball. The `.app` is a *directory*, so its
    // `metadata().len()` is the directory entry's size and not the bundle's — it is listed
    // anyway, because "the .app exists" is the fact worth printing and the `.dmg` beside it
    // carries the number that means something.
    for sub in ["appimage", "deb", "macos", "dmg", "tarball", "src"] {
        let Ok(entries) = fs::read_dir(bundle.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            if is_artefact(&entry.file_name().to_string_lossy())
                && let Ok(meta) = entry.metadata()
            {
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

/// Whether a file under `target/release/bundle/*` is one of the things this task produced.
///
/// Over the whole file name rather than `Path::extension`, which answers `"gz"` for
/// `cide-0.1.0-src.tar.gz`. The obvious fix — adding `"gz"` to the extension list — would be
/// wrong in the other direction, reporting any stray `.gz` in that tree as a release asset; and
/// leaving it out is worse still, because the listing is what a release script reads, so the
/// tarball would be built, never printed, and never uploaded.
fn is_artefact(file_name: &str) -> bool {
    file_name.ends_with(".tar.gz")
        || Path::new(file_name)
            .extension()
            .is_some_and(|e| e == "AppImage" || e == "deb" || e == "app" || e == "dmg")
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
    /// Skip, with a printed reason, when [`Self::program`] is not on `PATH`.
    ///
    /// For the steps whose tool is genuinely optional — today that is Flatpak, which the
    /// preflight already reports as a *warning* rather than a failure because the manifest is
    /// checked in and worth validating on a machine that cannot build it.
    ///
    /// It was a warning and then the run tried anyway, and `Command::status` on a missing
    /// program is `No such file or directory (os error 2)` — a message that names neither the
    /// tool nor the fact that everything before it succeeded. A user whose AppImage and .deb
    /// had both been produced was told the run had failed.
    optional: bool,
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
        // Resolved before it is run, so a missing tool is named rather than reported as
        // `No such file or directory (os error 2)` — which is what `Command::status` gives for
        // a program that does not exist, and which reads as a missing *file in the command*.
        if which(&self.program).is_none() {
            if self.optional {
                println!(
                    "\n· skipped: {} — `{}` is not on PATH",
                    self.display(),
                    self.program
                );
                return Ok(());
            }
            bail!(
                "`{}` is not on PATH, so this step cannot run: {}",
                self.program,
                self.display()
            );
        }
        println!("\n$ {}", self.display());
        let status = Command::new(&self.program)
            .args(&self.args)
            // xtask runs under *this* workspace's cargo, and cargo exports the toolchain
            // `rust-toolchain.toml` pins as `RUSTUP_TOOLCHAIN` to every child. A step that
            // builds a *different* workspace — the rust-analyzer fork, whose `rust-version`
            // is years ahead of cide's pin — must resolve its own toolchain from its own
            // directory, or it fails with "requires rustc 1.xx" against a compiler nobody
            // chose on purpose.
            .env_remove("RUSTUP_TOOLCHAIN")
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

    // First, and deliberately. It costs a second and compiles nothing, while everything below
    // it is a twenty-minute build that can die inside a bundler; a release that lost its
    // AppImage should still have the source it was built from. It is also the one step whose
    // *inputs* are already fixed — `git archive HEAD` reads the object database, not the working
    // tree — so running it before or after a build cannot change its bytes.
    /*
     * ...and skipped, not merely warned about, when a DEFAULT run meets a dirty tree.
     *
     * `src_verdicts` downgrades that case from `Fail` to `Warn` so the AppImage somebody asked
     * for still builds. A warning that left the archive step in the plan would then produce the
     * exact artefact the warning is about: a tarball claiming a commit whose contents it does not
     * have. The two halves are one decision and have to agree.
     *
     * `dirty_total == 0`, not `!dirty_total > 0` — `!` on a `usize` is bitwise NOT in Rust, so
     * that spelling compiles, reads as the negation, and is true for every value but `usize::MAX`.
     */
    if archives_source(targets, read_src_status(root).dirty_total) {
        steps.extend(src_steps(info));
    }

    if let Some(bundles) = targets.bundles() {
        // The sidecar, first. `cargo tauri build` builds only the app crate, so nothing else
        // in the plan would produce `cide-hook`, and the bundler's failure when the sidecar is
        // missing names a path with a target triple in it that reads like a cross-compilation
        // problem.
        steps.extend(sidecar_steps(triple));
        // The fork, for the same reason: it is not a workspace member, so no step below would
        // ever produce it, and the bundler needs its `-<triple>` copy to resolve externalBin.
        steps.extend(fork_steps(Some(triple), ra_rev(root).as_deref()));
        steps.extend(gopls_steps(Some(triple), gopls_rev(root).as_deref()));

        let mut env = Vec::new();
        if targets.appimage {
            let runtime = root.join(appimage_runtime_path(triple));
            if !runtime.exists() {
                steps.push(fetch_appimage_runtime(triple));
            }
            env.push((LDAI_RUNTIME_FILE.to_string(), runtime.display().to_string()));

            // The 32-bit-GTK trap. See `gtk_immodules_shim` for the whole chain; the short
            // version is that without this the bundle dies inside a downloaded shell script
            // and the only message that reaches the user is `failed to run linuxdeploy`.
            if let Some((target, link)) = gtk_immodules_shim(Path::new("/usr/bin")) {
                steps.push(link_gtk_immodules_shim(&target, &link));
                // Prepended, not appended: the whole point is to be found before the 32-bit
                // binary that `command -v` would otherwise return. Absolute, because the
                // process that reads it is several layers below this one — same reason the
                // runtime path above is absolute.
                let shim_dir = root.join(GTK_SHIM_DIR);
                let inherited = std::env::var("PATH").unwrap_or_default();
                env.push((
                    "PATH".to_string(),
                    format!("{}:{inherited}", shim_dir.display()),
                ));
            }
        }

        // Run from the app crate: `cargo tauri build` finds `tauri.conf.json` by walking up
        // from the working directory, and this workspace has no `src-tauri`.
        //
        // `beforeBuildCommand` in that config runs `pnpm build` in `ui/`, so the frontend is
        // not a separate step here — adding one would build it twice.
        steps.push(Step {
            program: "cargo".into(),
            args: vec![
                "tauri".into(),
                "build".into(),
                // The sidecar lives here and not in tauri.conf.json; see the module docs.
                // Without this flag the bundle comes out with no `cide-hook` and no error.
                "--config".into(),
                bundle_conf_arg(),
                "--bundles".into(),
                bundles,
            ],
            cwd: APP_CRATE.into(),
            env,
            optional: false,
        });
    }

    // After the bundler block, and that ordering is the whole reason this needs no build steps
    // of its own in the common case: `cargo tauri build` has already produced
    // `target/release/cide` and its `beforeBuildCommand` has already built `ui/dist`.
    if targets.tarball {
        // ...but with no Tauri bundle in the plan, nothing above built either of them, and
        // `cargo build -p cide-app` against an absent or stale `ui/dist` produces a binary that
        // launches to a blank window rather than an error. Conditional rather than
        // unconditional for the same reason the bundler step has no `pnpm build` beside it: a
        // plan that builds the frontend twice is a plan that misdescribes the run.
        if targets.bundles().is_none() {
            steps.extend(frontend_build_step(info));
            steps.push(release_binaries_step());
            // No triple copy: that name exists for the bundler, and this plan has none.
            steps.extend(fork_steps(None, ra_rev(root).as_deref()));
            steps.extend(gopls_steps(None, gopls_rev(root).as_deref()));
        }
        steps.extend(tarball_steps(info, triple));
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
            // The one optional tool. `flatpak-builder` is a separate package from `flatpak`
            // and is missing on an ordinary desktop; the AppImage and the .deb are already
            // built by the time this step is reached, so refusing to continue would report a
            // successful packaging run as a failure.
            optional: true,
        });
    }

    steps
}

/// Archive `HEAD` into `target/release/bundle/src/<product>-<version>-src.tar.gz`.
///
/// # Why `git archive` and not `tar`
///
/// The set of files is the question, and `git` already answers it: the archive is exactly the
/// tracked set at `HEAD`, so `.gitignore` decides what a release ships and there is one rule
/// rather than two that drift. A `tar --exclude` list would have to restate `target/`,
/// `node_modules/`, `ui/dist/` and `.claude/` and would silently start shipping the next
/// directory nobody remembered to add.
///
/// It also normalises what `tar` would not: every entry is `root/root`, mode 0644 or 0755, and
/// mtime = the commit date, so two clones of the same commit produce the same bytes. And the
/// tar stream opens with a `pax_global_header` carrying `comment=<commit sha>` — `git
/// get-tar-commit-id` reads it back — so the tarball self-identifies the commit it came from
/// without a synthesised `.commit` file.
///
/// # One step, not two, because of gzip's timestamp
///
/// `--format=tar.gz` compresses inside git, whose built-in gzip writes **MTIME 0** in the
/// header (measured: bytes 4–7 of the output are zero, and two runs two seconds apart are
/// byte-identical). Writing `foo.tar` and then running `gzip foo.tar` would not be: gzip stores
/// the *file's* mtime, so the same commit would produce a different sha256 every run and the
/// checksum on the release page would be reproducible by nobody. `gzip -n` fixes that, and this
/// form never needs it, which is the better kind of fix — there is no flag to forget.
///
/// # The `mkdir` is not decoration
///
/// `git archive -o` does not create its output directory; it exits 128 with
/// `could not open '…' for writing: No such file or directory`. Measured, not assumed.
fn src_steps(info: &AppInfo) -> Vec<Step> {
    vec![
        Step {
            program: "mkdir".into(),
            args: vec!["-p".into(), SRC_BUNDLE_DIR.into()],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        },
        Step {
            program: "git".into(),
            args: vec![
                "archive".into(),
                // One argument, so the printed plan stays pasteable.
                "--format=tar.gz".into(),
                format!("--prefix={}", src_prefix(info)),
                "-o".into(),
                src_archive_path(info),
                "HEAD".into(),
            ],
            cwd: ".".into(),
            env: Vec::new(),
            // Never optional: an absent `git` is a preflight *failure*, because skipping this
            // step would report a release run as complete with no source artefact in it.
            optional: false,
        },
    ]
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
            optional: false,
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
            optional: false,
        },
    ]
}

/// Where the suffixed copy of `cide-hook` goes, relative to the workspace root.
fn sidecar_path(triple: &str) -> String {
    format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}-{triple}")
}

/// Build the rust-analyzer fork from the sibling checkout. (M25)
///
/// Leaves the binary at `target/release/cide-rust-analyzer` — where the tarball stage and a
/// `current_exe().parent()` lookup both expect the name — and, when `triple` is given, makes
/// the `-<triple>` copy the Tauri bundler resolves `externalBin` through. The triple copy is
/// conditional for `release_binaries_step`'s reason inverted: a tarball-only plan with a
/// bundler-shaped file in it is a plan that has to be explained.
///
/// The build runs **in the sibling checkout**, against the fork's own lockfile and workspace.
/// The two `CARGO_PROFILE_*` variables are upstream's own dist settings (`xtask/src/dist.rs`
/// in the fork), stated here because this plan wants a bare binary rather than upstream's
/// zipped dist layout — and stated as env, not profile edits in the fork, so a rebase never
/// has to carry them. `CFG_RELEASE` is the same trick applied to identity: the fork's
/// `version.rs` reads it with `option_env!`, so `<rev>+cide` in `--version` output costs zero
/// diff in the fork. It is only ever for logs and bug reports — provenance in `cide-lsp` is
/// decided by *where the binary came from*, never by parsing this string.
fn fork_steps(triple: Option<&str>, rev: Option<&str>) -> Vec<Step> {
    let mut build_env = vec![
        ("CARGO_PROFILE_RELEASE_LTO".into(), "thin".into()),
        ("CARGO_PROFILE_RELEASE_CODEGEN_UNITS".into(), "1".into()),
    ];
    if let Some(rev) = rev {
        build_env.push(("CFG_RELEASE".into(), format!("{rev}+cide")));
    }
    let mut steps = vec![
        Step {
            program: "cargo".into(),
            args: vec![
                "build".into(),
                "--release".into(),
                "--locked".into(),
                "-p".into(),
                "rust-analyzer".into(),
            ],
            cwd: RA_FORK_DIR.into(),
            env: build_env,
            optional: false,
        },
        Step {
            program: "install".into(),
            args: vec![
                "-m755".into(),
                format!("{RA_FORK_DIR}/target/release/rust-analyzer"),
                format!("{HOOK_SIDECAR_DIR}/{RA_BIN}"),
            ],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        },
    ];
    if let Some(triple) = triple {
        steps.push(Step {
            program: "install".into(),
            args: vec![
                "-m755".into(),
                format!("{HOOK_SIDECAR_DIR}/{RA_BIN}"),
                ra_sidecar_path(triple),
            ],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        });
    }
    steps
}

/// Where the suffixed copy of the fork goes, mirroring [`sidecar_path`].
fn ra_sidecar_path(triple: &str) -> String {
    format!("{HOOK_SIDECAR_DIR}/{RA_BIN}-{triple}")
}

/// The pinned rev for the version stamp, or `None` when the lock is unreadable.
///
/// `Option` rather than an error because `plan` also runs for the printed no-`--run` plan and
/// for tests against roots that hold no lock — preflight is where an unreadable lock fails,
/// and a plan that merely lacks the stamp is still an honest plan.
fn ra_rev(root: &Path) -> Option<String> {
    read_ra_lock(root).ok().map(|lock| lock.rev)
}

/// The fork's preflight: the pin, the sibling checkout, and whether the two agree.
///
/// Absence is a **failure** and not `../cide-marketplace`'s skip, deliberately: the
/// marketplace tests skip because a test run without the sibling still proves everything it
/// claims to, while a bundle built without the fork ships without it — and the product then
/// falls back to the user's PATH rust-analyzer, which works, so nobody ever notices what the
/// package is missing. A wrong-rev checkout is only a warning, because a local `--run` on a
/// work-in-progress fork is a legitimate thing to do; release.yml always checks out the exact
/// pinned rev, so a release build never sees the warning.
fn fork_verdicts(root: &Path) -> Vec<Verdict> {
    let lock = match read_ra_lock(root) {
        Ok(lock) => lock,
        Err(error) => {
            return vec![Verdict::Fail(format!(
                "{RA_LOCK} could not be read ({error}). It pins which fork revision a package \
                 carries, and a package built from \"whatever is checked out\" is a claim \
                 nobody can reproduce"
            ))];
        }
    };
    let fork = root.join(RA_FORK_DIR);
    if !fork.join("Cargo.toml").exists() {
        return vec![Verdict::Fail(format!(
            "the rust-analyzer fork is not checked out at {RA_FORK_DIR}. Run: \
             git clone {url} {RA_FORK_DIR} && git -C {RA_FORK_DIR} checkout {rev}",
            url = lock.url,
            rev = lock.rev,
        ))];
    }
    // The fork's disk index rides a patched salsa: when its manifest carries the sibling
    // path patch, the build needs the salsa fork too, and dying inside cargo's patch
    // resolution twenty minutes into a bundler run is the failure this sentence pre-empts.
    // `../salsa` in the manifest resolves relative to the *fork*, so that is where the check
    // looks — resolving it against this repository was a latent bug that only worked while
    // everything happened to be siblings.
    if std::fs::read_to_string(fork.join("Cargo.toml"))
        .is_ok_and(|manifest| manifest.contains("salsa = { path = \"../salsa\" }"))
        && !fork.join("../salsa/Cargo.toml").exists()
    {
        return vec![Verdict::Fail(format!(
            "the rust-analyzer fork patches salsa to its sibling ../salsa, which is not \
             checked out. Clone the salsa fork beside the rust-analyzer fork (as \
             {RA_FORK_DIR}/../salsa) first",
        ))];
    }

    // HEAD against the pin, both resolved to commits **in the sibling**, so a tag in the lock
    // compares as the commit it names rather than as a string.
    let head = git_commit(&fork, "HEAD");
    let pinned = git_commit(&fork, &format!("{}^{{commit}}", lock.rev));
    vec![match (head, pinned) {
        (Some(head), Some(pinned)) if head == pinned => Verdict::Ok(format!(
            "{RA_FORK_DIR} is at the pinned fork revision ({})",
            lock.rev
        )),
        (Some(head), Some(_)) => Verdict::Warn(format!(
            "{RA_FORK_DIR} is at {} but {RA_LOCK} pins {} — fine for a local build, but this \
             package will not match what a release run would produce",
            &head[..head.len().min(12)],
            lock.rev,
        )),
        _ => Verdict::Warn(format!(
            "could not compare {RA_FORK_DIR}'s HEAD with the pinned revision {} — is {} \
             fetched there?",
            lock.rev, lock.rev,
        )),
    }]
}

/// Build the gopls sidecar from the pinned checkout and put it where the bundler looks.
///
/// `go build` in the module directory, not `go install pkg@version`: the install form needs
/// the network to even resolve the version and packages whatever the proxy serves, while the
/// checkout is the same auditable, patchable sibling the rust-analyzer fork established —
/// and it becomes an actual fork the day gopls needs a patch, with zero packaging change.
/// `CGO_ENABLED=0` because gopls is pure Go and a static binary sidesteps the AppImage/
/// tarball glibc coupling entirely. The `-ldflags -X` stamps the pin into `gopls version`
/// output — logs only; provenance is decided by where the binary came from, same as the
/// rust-analyzer rule.
fn gopls_steps(triple: Option<&str>, rev: Option<&str>) -> Vec<Step> {
    let mut args = vec!["build".to_string(), "-trimpath".to_string()];
    if let Some(rev) = rev {
        args.push("-ldflags".into());
        args.push(format!("-X main.version={rev}+cide"));
    }
    args.extend(["-o".to_string(), GOPLS_BIN.to_string(), ".".to_string()]);
    let mut steps = vec![
        Step {
            program: "go".into(),
            args,
            // The gopls module lives in the tools repository's `gopls/` directory.
            cwd: format!("{GOPLS_FORK_DIR}/gopls"),
            env: vec![("CGO_ENABLED".into(), "0".into())],
            optional: false,
        },
        Step {
            program: "install".into(),
            args: vec![
                "-m755".into(),
                format!("{GOPLS_FORK_DIR}/gopls/{GOPLS_BIN}"),
                format!("{HOOK_SIDECAR_DIR}/{GOPLS_BIN}"),
            ],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        },
    ];
    if let Some(triple) = triple {
        steps.push(Step {
            program: "install".into(),
            args: vec![
                "-m755".into(),
                format!("{HOOK_SIDECAR_DIR}/{GOPLS_BIN}"),
                gopls_sidecar_path(triple),
            ],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        });
    }
    steps
}

/// Where the suffixed copy of the gopls build goes, mirroring [`ra_sidecar_path`].
fn gopls_sidecar_path(triple: &str) -> String {
    format!("{HOOK_SIDECAR_DIR}/{GOPLS_BIN}-{triple}")
}

/// The pinned gopls rev for the version stamp, or `None` when the lock is unreadable.
fn gopls_rev(root: &Path) -> Option<String> {
    read_gopls_lock(root).ok().map(|lock| lock.rev)
}

/// The gopls checkout's preflight: [`fork_verdicts`]'s shapes, plus the Go toolchain.
///
/// Absence of the checkout is a failure for the same reason the rust-analyzer fork's is: the
/// package would build, ship without the sidecar, and fall back to the user's PATH gopls —
/// which works, so nobody notices. The Go toolchain check is the one new shape, and an *old*
/// `go` only warns: since 1.21 the go command auto-downloads the toolchain a module's `go`
/// directive demands (`GOTOOLCHAIN=auto`), so an older host go still builds the pin — it
/// just needs the network once to fetch the newer toolchain.
fn gopls_verdicts(root: &Path) -> Vec<Verdict> {
    let lock = match read_gopls_lock(root) {
        Ok(lock) => lock,
        Err(error) => {
            return vec![Verdict::Fail(format!(
                "{GOPLS_LOCK} could not be read ({error}). It pins which gopls revision a \
                 package carries, and a package built from \"whatever is checked out\" is a \
                 claim nobody can reproduce"
            ))];
        }
    };
    let checkout = root.join(GOPLS_FORK_DIR);
    if !checkout.join("gopls/go.mod").exists() {
        return vec![Verdict::Fail(format!(
            "the gopls checkout is not at {GOPLS_FORK_DIR}. Run: \
             git clone {url} {GOPLS_FORK_DIR} && git -C {GOPLS_FORK_DIR} checkout {rev}",
            url = lock.url,
            rev = lock.rev,
        ))];
    }
    let mut out = vec![go_toolchain_verdict(
        which("go").as_deref(),
        go_version_line().as_deref(),
        go_directive(&checkout.join("gopls/go.mod")).as_deref(),
    )];
    let head = git_commit(&checkout, "HEAD");
    let pinned = git_commit(&checkout, &format!("{}^{{commit}}", lock.rev));
    out.push(match (head, pinned) {
        (Some(head), Some(pinned)) if head == pinned => Verdict::Ok(format!(
            "{GOPLS_FORK_DIR} is at the pinned gopls revision ({})",
            lock.rev
        )),
        (Some(head), Some(_)) => Verdict::Warn(format!(
            "{GOPLS_FORK_DIR} is at {} but {GOPLS_LOCK} pins {} — fine for a local build, but \
             this package will not match what a release run would produce",
            &head[..head.len().min(12)],
            lock.rev,
        )),
        _ => Verdict::Warn(format!(
            "could not compare {GOPLS_FORK_DIR}'s HEAD with the pinned revision {} — is {} \
             fetched there?",
            lock.rev, lock.rev,
        )),
    });
    out
}

/// The Go-toolchain verdict, pure over pre-probed facts so the missing-tool sentences are
/// testable on a machine that has the tool — the same shape as `frontend_verdicts`.
fn go_toolchain_verdict(
    go: Option<&Path>,
    version_line: Option<&str>,
    directive: Option<&str>,
) -> Verdict {
    let Some(go) = go else {
        return Verdict::Fail(
            "`go` is not on PATH, and the gopls sidecar is built with it. Install Go \
             (https://go.dev/dl) or your distribution's `go` package"
                .into(),
        );
    };
    let host = version_line.and_then(parse_go_version);
    let needed = directive.and_then(parse_go_version);
    match (host, needed) {
        (Some(host), Some(needed)) if host < needed => Verdict::Warn(format!(
            "the host go is {} but the pinned gopls needs {} — `GOTOOLCHAIN=auto` will \
             download the newer toolchain during the build, which needs the network once",
            render_go_version(host),
            render_go_version(needed),
        )),
        _ => Verdict::Ok(format!("go is available at {}", go.display())),
    }
}

/// `"go version go1.25.5 linux/amd64"`, or `None` when `go` cannot be run.
fn go_version_line() -> Option<String> {
    let out = Command::new("go").arg("version").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `go 1.26.0` directive out of a `go.mod`, as its bare version string.
fn go_directive(go_mod: &Path) -> Option<String> {
    let text = fs::read_to_string(go_mod).ok()?;
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("go ")
            .map(|version| version.trim().to_string())
    })
}

/// `"go1.25.5"`, `"1.26.0"`, or a whole `go version` line → a comparable triple.
fn parse_go_version(text: &str) -> Option<(u32, u32, u32)> {
    let token = text.split_whitespace().find(|word| {
        word.trim_start_matches("go")
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let mut parts = token.trim_start_matches("go").split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

fn render_go_version((major, minor, patch): (u32, u32, u32)) -> String {
    format!("{major}.{minor}.{patch}")
}

/// One revision resolved to a commit id in a checkout, or `None` for anything else.
fn git_commit(repo: &Path, rev: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", rev])
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Run `build.beforeBuildCommand` the way `cargo tauri build` would, for a plan that has no
/// `cargo tauri build` in it.
///
/// `sh -c` with the whole line, and the config's own `cwd`, because that is literally what
/// tauri-cli does with it (`src/helpers/mod.rs:105`) — reimplementing it as `pnpm build` with a
/// hardcoded directory would be a second reading of the same config, and the day someone changes
/// `beforeBuildCommand` only one of the two would follow.
///
/// `None` when the config declares no hook. That is a legitimate arrangement — the frontend is
/// built by hand — and `frontend_verdicts` already warns about it; inventing a step here would
/// turn that warning into a failure to run a command nobody configured.
fn frontend_build_step(info: &AppInfo) -> Option<Step> {
    let build = info.before_build.as_ref()?;
    Some(Step {
        program: "sh".into(),
        args: vec!["-c".into(), build.script.clone()],
        cwd: frontend_dir(build),
        env: Vec::new(),
        optional: false,
    })
}

/// Build both binaries the tarball ships.
///
/// Only reached when no Tauri bundle was asked for; otherwise `cargo tauri build` has already
/// built the app and [`sidecar_steps`] the hook, and a second `cargo build` would be a no-op
/// line in a printed plan that suggests two compilations happen.
///
/// One step rather than reusing [`sidecar_steps`], even though that also builds `cide-hook`.
/// Its second command copies the binary to `cide-hook-<triple>`, which exists so
/// `tauri-bundler` can resolve `externalBin` — and a plan with no bundler in it that produces a
/// bundler-shaped file is a plan that has to be explained. The tarball puts `cide-hook` beside
/// `cide` under its own name, which is where `cide` looks for it.
fn release_binaries_step() -> Step {
    Step {
        program: "cargo".into(),
        args: vec![
            "build".into(),
            "--release".into(),
            "--locked".into(),
            "-p".into(),
            APP_PACKAGE.into(),
            "-p".into(),
            HOOK_BIN.into(),
        ],
        cwd: ".".into(),
        env: Vec::new(),
        optional: false,
    }
}

/// Assemble `target/release/tarball-stage/<prefix>/` and archive it.
///
/// # Why a staging tree and not `tar --transform`
///
/// `tar` can rewrite paths on the way in, and doing so would save these `install` calls. It
/// would also make the archive's layout a regex in an argument list, invisible until someone
/// unpacks the result — and it is GNU-only, which the rest of this module is careful not to be
/// where it has a choice. The staging tree is the layout, written out, and `ls -R` on it answers
/// "what is in the tarball" without unpacking anything.
///
/// # Why `install` and not `cp`
///
/// The same reason [`sidecar_steps`] uses it: the mode is stated rather than inherited, so the
/// binaries are executable in the archive even if the build left them otherwise, and the config
/// files are not. `install` also creates nothing but the file — the directories are made once,
/// up front, which is what keeps the printed plan short enough to read.
fn tarball_steps(info: &AppInfo, triple: &str) -> Vec<Step> {
    let prefix = tarball_prefix(info);
    let root = format!("{TARBALL_STAGE_DIR}/{prefix}");

    let mut dirs = vec![format!("{root}/bin"), format!("{root}/share/applications")];
    dirs.extend(
        TARBALL_ICON_SIZES
            .iter()
            .map(|n| format!("{root}/share/icons/hicolor/{n}x{n}/apps")),
    );

    // `rm -rf` first, in the same `sh -c` as the `mkdir -p`: the staging directory survives a
    // run, so a second build with a different version — or one where a file was renamed — would
    // otherwise archive both the new tree and whatever the last run left beside it.
    let mut steps = vec![Step {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "rm -rf '{TARBALL_STAGE_DIR}' && mkdir -p {}",
                dirs.iter()
                    .map(|d| format!("'{d}'"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        ],
        cwd: ".".into(),
        env: Vec::new(),
        optional: false,
    }];

    let mut install = |mode: &str, from: String, to: String| {
        steps.push(Step {
            program: "install".into(),
            args: vec![mode.into(), from, to],
            cwd: ".".into(),
            env: Vec::new(),
            optional: false,
        });
    };

    install(
        "-m755",
        format!("{HOOK_SIDECAR_DIR}/{APP_BIN}"),
        format!("{root}/bin/{APP_BIN}"),
    );
    // The hook, and not as a `-<triple>` sidecar copy: that name exists so `tauri-bundler` can
    // resolve `externalBin`, and outside a bundle it is noise. `cide` looks for `cide-hook`
    // beside itself.
    install(
        "-m755",
        format!("{HOOK_SIDECAR_DIR}/{HOOK_BIN}"),
        format!("{root}/bin/{HOOK_BIN}"),
    );
    // The rust-analyzer fork, under the same roof rule: `cide_core::toolchain::sibling_binary`
    // looks for `cide-rust-analyzer` beside `cide`. Its distinct file name is what makes
    // "put bin/ on your PATH" safe — see RA_BIN.
    install(
        "-m755",
        format!("{HOOK_SIDECAR_DIR}/{RA_BIN}"),
        format!("{root}/bin/{RA_BIN}"),
    );
    install(
        "-m755",
        format!("{HOOK_SIDECAR_DIR}/{GOPLS_BIN}"),
        format!("{root}/bin/{GOPLS_BIN}"),
    );

    // The generated desktop entry rather than one written here: it is derived from `AppInfo` and
    // `package --check` keeps it in sync, so there is one source for what the entry says. Its
    // `Exec=cide %U` is unqualified, which is the tarball's one instruction to the person
    // unpacking it — `bin/` has to be on `PATH` for the entry to launch anything.
    install(
        "-m644",
        format!("{FLATPAK_DIR}/{}.desktop", info.identifier),
        format!("{root}/share/applications/{}.desktop", info.identifier),
    );

    for n in TARBALL_ICON_SIZES {
        install(
            "-m644",
            format!("{APP_CRATE}/icons/{n}x{n}.png"),
            format!(
                "{root}/share/icons/hicolor/{n}x{n}/apps/{}.png",
                info.identifier
            ),
        );
    }

    // Both, and neither is decoration. `LICENSE` is the licence this archive is distributed
    // under and a binary distribution has to carry it; `README.md` is where "it needs WebKitGTK
    // 4.1" is written down, which is the first thing that goes wrong for someone who unpacked
    // this instead of downloading the AppImage.
    install("-m644", "LICENSE".into(), format!("{root}/LICENSE"));
    install("-m644", "README.md".into(), format!("{root}/README.md"));

    steps.push(Step {
        program: "mkdir".into(),
        args: vec!["-p".into(), TARBALL_BUNDLE_DIR.into()],
        cwd: ".".into(),
        env: Vec::new(),
        optional: false,
    });

    steps.push(Step {
        program: "tar".into(),
        args: vec![
            // Sorted, and owned by uid/gid 0 with no name lookup, so the archive does not carry
            // the build machine's user in it and two builds of the same tree differ only where
            // the compiler made them differ. That is tidiness, NOT the reproducibility promise
            // `--src` makes: this contains rustc output and its sha256 describes one build.
            "--sort=name".into(),
            "--owner=0".into(),
            "--group=0".into(),
            "--numeric-owner".into(),
            "-czf".into(),
            tarball_path(info, triple),
            "-C".into(),
            TARBALL_STAGE_DIR.into(),
            prefix,
        ],
        cwd: ".".into(),
        env: Vec::new(),
        optional: false,
    });

    steps
}

/// The environment variable `linuxdeploy-plugin-appimage` turns into `--runtime-file`.
const LDAI_RUNTIME_FILE: &str = "LDAI_RUNTIME_FILE";

/// Where a 64-bit `gtk-query-immodules-3.0` is put when the host's unsuffixed one is 32-bit.
const GTK_SHIM_DIR: &str = "target/appimage-gtk-shim";

/// The tool `linuxdeploy-plugin-gtk` runs to build the AppDir's `immodules.cache`.
const GTK_IMMODULES_TOOL: &str = "gtk-query-immodules-3.0";

/// Is this file an ELF64 image?
///
/// Byte 4 of an ELF header is `EI_CLASS`: 1 is 32-bit, 2 is 64-bit. Read rather than shelled
/// out to `file(1)`, which is not guaranteed present and whose output is prose.
fn is_elf64(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    bytes.len() > 4 && bytes[..4] == [0x7f, b'E', b'L', b'F'] && bytes[4] == 2
}

/// The multilib trap that makes an AppImage build fail with nothing but `failed to run
/// linuxdeploy`, and the shim that gets around it.
///
/// On a distribution that ships 32-bit GTK alongside 64-bit — openSUSE's `gtk3-tools-32bit` is
/// the case this was found on — the **32-bit** package owns the unsuffixed
/// `/usr/bin/gtk-query-immodules-3.0` and the 64-bit tool is renamed to `…-3.0-64`.
/// `linuxdeploy-plugin-gtk`'s `search_tool` tries `command -v` *first* and returns on the first
/// hit, so it finds the 32-bit binary and never reaches the `/usr/bin/$tool-64` entry that is
/// already in its own fallback list. It then runs that binary over the AppDir's 64-bit
/// immodules, every load fails with `wrong ELF class: ELFCLASS64`, the tool exits 1, and the
/// plugin — which is `set -e` — dies. linuxdeploy reports `Failed to run plugin: gtk`, and
/// tauri-bundler discards linuxdeploy's stderr at its default log level, so all the user is
/// told is `failed to bundle project: failed to run linuxdeploy`.
///
/// Nothing about this is cide's, and it is not fixable in cide's source: the decision is made
/// inside a downloaded shell script. What *is* available is `PATH`, which that `command -v`
/// honours. So a directory holding one symlink named `gtk-query-immodules-3.0` and pointing at
/// the 64-bit tool goes on the front of the step's `PATH`, and `search_tool` finds the right
/// binary by the same rule it was already using.
///
/// `None` when the host is not in this state — the common case, where the unsuffixed tool is
/// already 64-bit or absent. A shim is never installed speculatively: putting a directory in
/// front of `PATH` for every build would be a durable hazard in exchange for nothing.
fn gtk_immodules_shim(bin_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let unsuffixed = bin_dir.join(GTK_IMMODULES_TOOL);
    let suffixed = bin_dir.join(format!("{GTK_IMMODULES_TOOL}-64"));

    // Only when the unsuffixed one exists *and* is the wrong class. If it is missing entirely
    // the plugin's own fallback list reaches `-64` unaided, and if it is already 64-bit there
    // is nothing to fix.
    if !unsuffixed.exists() || is_elf64(&unsuffixed) || !is_elf64(&suffixed) {
        return None;
    }
    Some((
        suffixed,
        PathBuf::from(GTK_SHIM_DIR).join(GTK_IMMODULES_TOOL),
    ))
}

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
/// Put the 64-bit `gtk-query-immodules-3.0` where a `PATH` lookup will find it first.
///
/// `ln -sfn` rather than a copy: the tool is part of the host's GTK installation and must track
/// it, and `-f` makes a second build over an existing link a no-op rather than an error.
/// `mkdir -p` first, in one `sh -c`, because `ln` will not create the directory and a plan of
/// two steps for one symlink reads like two things going on.
fn link_gtk_immodules_shim(target: &Path, link: &Path) -> Step {
    Step {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "mkdir -p {} && ln -sfn {} {}",
                shell_quote(
                    &link
                        .parent()
                        .unwrap_or(Path::new("."))
                        .display()
                        .to_string()
                ),
                shell_quote(&target.display().to_string()),
                shell_quote(&link.display().to_string()),
            ),
        ],
        cwd: ".".into(),
        env: Vec::new(),
        optional: false,
    }
}

/// Single-quote one argument for `sh -c`.
///
/// These paths are `/usr/bin/...` and a path under `target/` today, but this builds a shell
/// command line and a quoting rule that is only correct for the paths it happens to see is the
/// kind that stops being correct silently.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

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
        optional: false,
    }
}

/// The manifest's path, relative to the workspace root.
fn manifest_path(info: &AppInfo) -> String {
    format!("{FLATPAK_DIR}/{}.yml", info.identifier)
}

/// Everything that can be checked without building.
///
/// `triple` is the host's, from `rustc -vV`. It is a parameter and not a `cfg!` so that both
/// hosts' verdicts can be exercised from one machine — which for the macOS half is the *only*
/// way they can be exercised at all.
pub fn preflight(root: &Path, info: &AppInfo, targets: Targets, triple: &str) -> Vec<Verdict> {
    let mut out = Vec::new();

    // First, before anything that would be wasted effort. A target the host cannot produce is
    // not a configuration problem to be reported alongside twelve others; it is the answer.
    let impossible = targets.impossible_on(triple);
    if !impossible.is_empty() {
        let (needed, why) = if is_macos_triple(triple) {
            (
                "Linux",
                "`linuxdeploy` and `dpkg-deb` are Linux tools and the AppImage carries a Linux \
                 WebKitGTK",
            )
        } else {
            (
                "macOS",
                "linking Cocoa/WebKit needs Apple's linker and SDK, which are licensed to Apple \
                 hardware, and `codesign`/`hdiutil` exist nowhere else",
            )
        };
        out.push(Verdict::Fail(format!(
            "this host is {triple}; {} can only be built on {needed}. Cross-compilation is not \
             possible — {why}. Use a {needed} machine or a CI runner",
            impossible.join(", "),
        )));
    }

    // Which file has to declare a target is decided by the **target's** platform, not by the
    // host's. The macOS overlay *replaces* `bundle.targets` (the merge is RFC 7386, in which an
    // array is a scalar), so `dmg` is never going to appear in the base config and asking the
    // base config about it would produce a second, misleading failure beside the honest one
    // above — which is what the first version of this did.
    for (wanted, name, macos_only) in [
        (targets.appimage, "appimage", false),
        (targets.deb, "deb", false),
        (targets.app, "app", true),
        (targets.dmg, "dmg", true),
    ] {
        if !wanted {
            continue;
        }
        let (declares, declared_in) = if macos_only {
            (&info.macos_bundle_targets, TAURI_MACOS_CONF)
        } else {
            (&info.bundle_targets, TAURI_CONF)
        };
        if declares.iter().any(|t| t == name) {
            out.push(Verdict::Ok(format!("{declared_in} bundles `{name}`")));
        } else {
            out.push(Verdict::Fail(format!(
                "{declared_in} does not list `{name}` in bundle.targets, so \
                 `cargo tauri build` will not produce one"
            )));
        }
    }

    // The union of the icon lists that could be in force, deduplicated. A union rather than a
    // choice because the question here is only "does this file exist", and the two lists name
    // the same directory: reporting one icon twice would be noise, and reporting neither list
    // when the *other* platform's was asked for would leave a missing raster undetected.
    for (icon, declared_in) in icon_list(info, targets) {
        let path = root.join(APP_CRATE).join(icon);
        if path.exists() {
            out.push(Verdict::Ok(format!("icon {icon}")));
        } else {
            out.push(Verdict::Fail(format!(
                "icon {icon} is named in {declared_in} but missing at {}",
                path.display()
            )));
        }
    }

    if targets.app || targets.dmg {
        out.extend(macos_checks(info));
    }

    if targets.appimage || targets.deb || targets.app || targets.dmg {
        out.push(tauri_cli_check());
        // Beside the CLI check and not below the sidecar one, because this is the *first* thing
        // `cargo tauri build` does — before it compiles a line of Rust. A machine that cannot run
        // the frontend build cannot produce any bundle at all, on either platform.
        out.extend(frontend_checks(root, info));
        out.push(if info.bundles_the_hook() {
            Verdict::Ok(format!(
                "{TAURI_BUNDLE_CONF} ships {HOOK_BIN} as a sidecar (bundle.externalBin)"
            ))
        } else {
            // A failure, not a warning. The bundle would be produced, would install, would
            // launch, and every session in it would run with no hooks — see the module docs.
            Verdict::Fail(format!(
                "{TAURI_BUNDLE_CONF} has no `{HOOK_BIN}` in bundle.externalBin, so the package \
                 would ship without it: `cargo tauri build` bundles only the app crate's own \
                 binaries. Sessions would run with no hooks and nothing would report an error"
            ))
        });
        out.push(if info.bundles_the_fork() {
            Verdict::Ok(format!(
                "{TAURI_BUNDLE_CONF} ships {RA_BIN} as a sidecar (bundle.externalBin)"
            ))
        } else {
            // The same failure class as the hook's, with a quieter symptom: the product falls
            // back to the user's PATH rust-analyzer, which works, so the missing binary is
            // invisible until somebody asks why their index is not on disk.
            Verdict::Fail(format!(
                "{TAURI_BUNDLE_CONF} has no `{RA_BIN}` in bundle.externalBin, so the package \
                 would ship without cide's rust-analyzer and silently fall back to PATH"
            ))
        });
        out.extend(fork_verdicts(root));
        out.push(if info.bundles_the_gopls() {
            Verdict::Ok(format!(
                "{TAURI_BUNDLE_CONF} ships {GOPLS_BIN} as a sidecar (bundle.externalBin)"
            ))
        } else {
            Verdict::Fail(format!(
                "{TAURI_BUNDLE_CONF} has no `{GOPLS_BIN}` in bundle.externalBin, so the \
                 package would ship without cide's gopls and silently fall back to PATH"
            ))
        });
        out.extend(gopls_verdicts(root));
        // The mirror image, and the more expensive mistake of the two: `tauri-build` acts on
        // `bundle.externalBin` on every cargo invocation, so the same key in the *base* config
        // fails `cargo build --workspace` on any tree that has not already produced a release
        // sidecar. That is a broken checkout and a red CI, not a broken package.
        if !info.base_external_bin.is_empty() {
            out.push(Verdict::Fail(format!(
                "{TAURI_CONF} carries bundle.externalBin ({:?}). `tauri-build` copies those on \
                 every `cargo build`, so the whole workspace stops compiling until a release \
                 {HOOK_BIN} exists. Move it to {TAURI_BUNDLE_CONF}, which only the bundler step \
                 reads (`--config`)",
                info.base_external_bin
            )));
        }
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
    } else if targets.tarball {
        // A tarball-only run has no `cargo tauri build` in it, so none of the block above
        // applies: no CLI version to check, and no sidecar question, because `externalBin` is a
        // bundler concept and the tarball simply copies `cide-hook` in beside `cide`.
        //
        // What does still apply is the frontend, because `plan` runs `beforeBuildCommand` itself
        // in that case (see `frontend_build_step`). Without this, `--tarball` on a machine with
        // no `ui/node_modules` passes preflight and then fails several minutes later inside
        // `cargo build`, with "Unable to find your web assets" and no hint that `pnpm install`
        // was the missing step.
        out.extend(frontend_checks(root, info));
        // And both forks, which the tarball carries as `bin/` binaries with no bundler
        // involved — so the externalBin question does not apply but the checkout ones do.
        out.extend(fork_verdicts(root));
        out.extend(gopls_verdicts(root));
    }

    if targets.appimage {
        out.push(webkit_helper_check(info));
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
        let generated_set = match (read_ra_lock(root), read_gopls_lock(root)) {
            (Ok(ra), Ok(gopls)) => generated(info, &ra, &gopls),
            (Err(error), _) => {
                out.push(Verdict::Fail(format!(
                    "{RA_LOCK} could not be read ({error}), so the generated manifest cannot \
                     be checked — it embeds the fork pin"
                )));
                Vec::new()
            }
            (_, Err(error)) => {
                out.push(Verdict::Fail(format!(
                    "{GOPLS_LOCK} could not be read ({error}), so the generated manifest \
                     cannot be checked — it embeds the gopls pin"
                )));
                Vec::new()
            }
        };
        for (relative, expected) in generated_set {
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

    if targets.src {
        out.extend(src_checks(root, info, targets.src_named));
    }

    out
}

/// The environment variables `tauri-bundler` reads before it signs and notarises a macOS bundle.
///
/// Named here rather than looked up in three places, and checked *by name* so the preflight can
/// tell "you have not set this up" from "you set it up wrong": the bundler's own failure for a
/// missing `APPLE_ID` arrives after the `.app` has been built, minutes in.
const MACOS_SIGNING_VARS: [&str; 2] = ["APPLE_SIGNING_IDENTITY", "APPLE_CERTIFICATE"];
const MACOS_NOTARY_VARS: [&str; 3] = ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"];

/// The macOS-only preflight: the overlay, the sidecar rule that applies there, and signing.
///
/// **Every verdict here was written from `tauri-utils`' schema and `tauri-bundler`'s macOS
/// bundler, not from a build.** Nobody has run this. It is still worth having, because the two
/// things it catches are both silent: a `.app` built with no code signature at all, which
/// downloads with a quarantine bit and refuses to open with *"cide is damaged and can't be
/// opened"* — a message that blames the download rather than the missing signature — and an
/// overlay key that would break `cargo build` on a Mac while leaving Linux green.
fn macos_checks(info: &AppInfo) -> Vec<Verdict> {
    let mut out = Vec::new();

    out.push(if info.macos_bundle_targets.is_empty() {
        Verdict::Fail(format!(
            "{TAURI_MACOS_CONF} has no bundle.targets. The base config asks for `appimage` and \
             `deb`, which a Mac cannot build, so `cargo tauri build` would produce nothing and \
             say so only at the end"
        ))
    } else {
        Verdict::Ok(format!(
            "{TAURI_MACOS_CONF} overrides bundle.targets with {:?}",
            info.macos_bundle_targets
        ))
    });

    // The same trap as `base_external_bin`, one platform over and invisible from here.
    // `tauri-build` reads the *host's* platform overlay on every cargo invocation, so this key
    // in this file would fail `cargo build --workspace` on a Mac with a missing-resource error,
    // while every gate on this machine stayed green.
    if !info.macos_external_bin.is_empty() {
        out.push(Verdict::Fail(format!(
            "{TAURI_MACOS_CONF} carries bundle.externalBin ({:?}). `tauri-build` reads the host's \
             platform overlay on every `cargo build`, so this breaks the whole workspace build on \
             macOS — and only there. The sidecar belongs in {TAURI_BUNDLE_CONF}, which nothing \
             reads unless the bundler step asks for it",
            info.macos_external_bin
        )));
    }

    out.extend(signing_verdicts(
        &MACOS_SIGNING_VARS
            .into_iter()
            .filter(|v| std::env::var_os(v).is_some())
            .collect::<Vec<_>>(),
        &MACOS_NOTARY_VARS
            .into_iter()
            .filter(|v| std::env::var_os(v).is_none())
            .collect::<Vec<_>>(),
    ));

    // The updater's macOS channel is a different artefact from the `.dmg`. Worth one line
    // because the Linux warning below names AppImage and would otherwise read as "sorted".
    if !info.has_updater {
        out.push(Verdict::Warn(
            "no `plugins.updater` in the config. On macOS the updater's artefact is an \
             `app.tar.gz`, which is a third bundle target rather than a property of the .dmg — \
             so self-updating is not something the .dmg gains by being configured"
                .into(),
        ));
    }

    out
}

/// The signing and notarisation verdicts, over the environment rather than reading it.
///
/// Pure so the assertions about it are deterministic: reading the process environment inside a
/// verdict makes the test that checks the wording pass or fail depending on whose machine it
/// runs on, and a signing certificate on a developer's Mac is exactly the case where a wrong
/// message costs the most.
fn signing_verdicts(configured: &[&str], notary_unset: &[&str]) -> Vec<Verdict> {
    let mut out = Vec::new();
    out.push(if configured.is_empty() {
        // A warning and not a failure, deliberately. An unsigned local build is a legitimate
        // thing to want and is the only thing available without a paid Apple Developer Program
        // membership; refusing it would make this task unusable for the first Mac build, which
        // is the one that matters most.
        Verdict::Warn(format!(
            "none of {MACOS_SIGNING_VARS:?} is set, so the bundle will be unsigned. It runs \
             locally, but a downloaded copy is quarantined and Gatekeeper reports it as damaged \
             rather than as unidentified; a user has to `xattr -dr com.apple.quarantine` it. A \
             Developer ID certificate needs a paid Apple Developer Program membership"
        ))
    } else {
        Verdict::Ok(format!(
            "code signing configured via {}",
            configured.join(", ")
        ))
    });

    // Only worth saying once there is a signature to notarise. Said unconditionally it is noise
    // on every unsigned build, and a preflight people learn to skip is worse than one that says
    // less.
    if !notary_unset.is_empty() && !configured.is_empty() {
        out.push(Verdict::Warn(format!(
            "signing is configured but {notary_unset:?} {} unset, so the bundle will not be \
             notarised and Gatekeeper will still warn on a downloaded copy",
            if notary_unset.len() == 1 { "is" } else { "are" }
        )));
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
/// **This is the check that catches the quietest failure in this whole file**, and its first
/// version got the reason wrong in a way worth recording, because the wrong reason made it a
/// warning when it should have been a failure.
///
/// The bundler puts `libwebkit2gtk-4.1.so.0` inside the AppImage, and that library spawns
/// helper executables to do anything at all. It tries to bring them along — it looks under
/// `<libdir>/webkit2gtk-4.1/` — and when it does not find them it copies nothing and says
/// nothing: the loop is `if source.exists()`, with no else. On openSUSE they live in
/// `/usr/libexec/libwebkit2gtk-4_1-0/`, which is not a name on that list.
///
/// The old explanation said the library then "falls back to its compiled-in absolute path", so
/// the package would still run on a host laid out like the build machine. Both halves are
/// false, and the observed crash is the proof:
///
/// ```text
/// Failed to spawn child process “././/libexec/libwebkit2gtk-4_1-0/WebKitNetworkProcess”
/// ```
///
/// That path is **relative**, and WebKitGTK resolves it against the directory the library
/// itself is loaded from. On the system, `/usr/lib64/` + `../libexec/…` is `/usr/libexec/…`
/// and everything works; inside the AppImage, `<AppDir>/usr/lib/` + `../libexec/…` is
/// `<AppDir>/usr/libexec/…`, which does not exist. So the package fails **on the build machine
/// too** — there is no host it runs on — and it aborts before the first frame rather than
/// showing a window that never paints.
///
/// `WEBKIT_EXEC_PATH` was the other half of the old note. That variable is not referenced by
/// this WebKit at all (checked with `strings` against the bundled 2.52 library), so setting it
/// changes nothing; it was tried against the failing AppImage and the crash was identical.
///
/// The fix is therefore to place the helpers where the relative path lands, which is what
/// `bundle.linux.appimage.files` does. Verified by copying them into an extracted AppDir and
/// running `AppRun`: the window opened and the IPC probe reported the fast path.
fn webkit_helper_check(info: &AppInfo) -> Verdict {
    let Some(dir) = find_webkit_helpers_elsewhere() else {
        // Nothing to copy from. The bundler's own search may still find them, in which case
        // this is fine and the mapping below would be pointing at nothing.
        return Verdict::Ok("WebKit's helper processes are where the bundler looks".into());
    };

    let missing: Vec<&str> = WEBKIT_HELPERS
        .into_iter()
        .filter(|helper| {
            !info
                .appimage_files
                .iter()
                .any(|(target, _)| target.ends_with(&format!("/{helper}")))
        })
        .collect();
    if missing.is_empty() {
        // And the sources have to be real, or the bundler copies nothing and says nothing —
        // the same silence this check exists for, one layer up.
        let absent: Vec<&str> = info
            .appimage_files
            .iter()
            .filter(|(_, source)| !Path::new(source).exists())
            .map(|(_, source)| source.as_str())
            .collect();
        if absent.is_empty() {
            return Verdict::Ok(format!(
                "the AppImage maps WebKit's helper processes from {dir} into usr/libexec/"
            ));
        }
        return Verdict::Fail(format!(
            "bundle.linux.appimage.files names {} which does not exist, so the bundler will \
             copy nothing and the AppImage will abort on launch",
            absent.join(", ")
        ));
    }

    Verdict::Fail(format!(
        "the AppImage will not contain {}, and will abort before its first frame: the bundled \
         libwebkit2gtk resolves `././/libexec/libwebkit2gtk-4_1-0` against its own directory, \
         so inside the package that is <AppDir>/usr/libexec/, which nothing populates. This \
         machine keeps them in {dir} — map them in with bundle.linux.appimage.files in {}",
        missing.join(" and "),
        TAURI_BUNDLE_CONF,
    ))
}

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

/// Whether this machine can run the frontend build, which is the bundler's first step.
///
/// # This is what the first Mac packaging run actually died on, and it died late
///
/// `cargo tauri build` runs `build.beforeBuildCommand` through `sh -c` before it compiles
/// anything (`tauri-cli-2.11.4` `src/helpers/mod.rs:105`). On a Mac with no `pnpm` that is:
///
/// ```text
/// Running beforeBuildCommand `pnpm build`
/// sh: pnpm: command not found
/// Error beforeBuildCommand `pnpm build` failed with exit code 127
/// ```
///
/// — and it arrived *after* `cargo build --release -p cide-hook`, because that is step one of the
/// plan and this is step three. Every fact needed to predict it was available in the first
/// second: the config names the command and `PATH` says whether it exists. The preflight had a
/// verdict for `cargo-tauri` and none for the tool the build reaches for first, so the run spent
/// a release build to report a missing package manager.
///
/// The Rust half of the toolchain is not checked here for the same reason `cargo` itself is not:
/// this task is *running under* cargo, so its presence is not in question. The frontend half can
/// be absent on a machine that runs this command perfectly well — and on a fresh clone it is
/// absent twice over, once for the tool and once for `node_modules`. Both are reported in one
/// pass, so a Mac that has neither needs one round trip rather than two long ones.
fn frontend_checks(root: &Path, info: &AppInfo) -> Vec<Verdict> {
    let Some(build) = info.before_build.as_ref() else {
        // Not a failure: a config with no hook is a legitimate arrangement — it means the
        // frontend is built by hand — and `ui/dist` may well be sitting there already. It is a
        // warning because nothing in the plan then rebuilds it, so the bundle can silently carry
        // the frontend from whenever somebody last ran `pnpm build`.
        return vec![Verdict::Warn(format!(
            "{TAURI_CONF} has no build.beforeBuildCommand, so nothing in this plan rebuilds the \
             frontend: the bundle will carry whatever `ui/dist` already holds, and `cargo tauri \
             build` fails with \"Unable to find your web assets\" if it holds nothing"
        ))];
    };

    // Resolved the way `tauri-cli` resolves it — against the directory the bundler step runs
    // from, which is `APP_CRATE`. See `BeforeBuild::cwd`.
    let dir = root
        .join(APP_CRATE)
        .join(build.cwd.as_deref().unwrap_or("."));
    frontend_verdicts(
        build,
        frontend_tool(&build.script).and_then(which),
        // `None` — nothing to say — unless there is a `package.json` to have installed from. The
        // question is about *this* frontend; a `beforeBuildCommand` that is not a node build has
        // no `node_modules` to be missing, and inventing a failure for it would refuse a build
        // that would have worked.
        dir.join("package.json")
            .is_file()
            .then(|| dir.join("node_modules").is_dir()),
    )
}

/// The wording, over facts rather than over the machine.
///
/// Split from [`frontend_checks`] the way [`signing_verdicts`] is split from its caller, and for
/// the same reason: these sentences are the whole value of the check, and a verdict that reads
/// `PATH` while it decides what to say can only be asserted on a machine that happens to be
/// missing the tool — which is to say, never on the machine this is developed on.
fn frontend_verdicts(
    build: &BeforeBuild,
    tool_path: Option<PathBuf>,
    deps_installed: Option<bool>,
) -> Vec<Verdict> {
    let mut out = Vec::new();
    let dir = frontend_dir(build);
    let script = &build.script;

    match (frontend_tool(script), tool_path) {
        (Some(tool), Some(path)) => out.push(Verdict::Ok(format!(
            "build.beforeBuildCommand is `{script}` in {dir}; {tool} at {}",
            path.display()
        ))),
        (Some(tool), None) => {
            // Named for `pnpm` only, because that is what this repository ships and a guessed
            // install line for some other tool would be advice nobody checked.
            let install = if tool == "pnpm" {
                ". Install it with `corepack enable pnpm` (corepack ships with Node), \
                 `brew install pnpm`, or `npm install -g pnpm`"
            } else {
                ""
            };
            out.push(Verdict::Fail(format!(
                "`{tool}` is not on PATH, and {TAURI_CONF} runs `{script}` in {dir} as \
                 build.beforeBuildCommand. `cargo tauri build` runs that line through `sh -c` \
                 before it compiles anything, so the run ends in `sh: {tool}: command not found` \
                 and `beforeBuildCommand failed with exit code 127` — after the release build of \
                 {HOOK_BIN} has already been spent{install}"
            )));
        }
        (None, _) => out.push(Verdict::Warn(format!(
            "build.beforeBuildCommand is `{script}`, which is not a plain `program args…` line, \
             so which program it needs was not checked. If the build dies with `command not \
             found`, that is what it means"
        ))),
    }

    match deps_installed {
        Some(false) => {
            let install = match frontend_tool(script) {
                // `--dir` is pnpm's spelling (npm's is `--prefix`), so the run-it-from-the-root
                // form — which is how README and CLAUDE.md write it — is only offered for the
                // tool this repository actually ships.
                Some("pnpm") => format!("Run `pnpm --dir {dir} install`"),
                Some(tool) => format!("Run `{tool} install` in {dir}/"),
                None => format!("Install the frontend's dependencies in {dir}/"),
            };
            out.push(Verdict::Fail(format!(
                "{dir}/node_modules does not exist, so `{script}` has nothing to run with: the \
                 tools its package.json script invokes live there, and it will exit before \
                 anything is written to the frontend's dist directory. {install}"
            )));
        }
        Some(true) => out.push(Verdict::Ok(format!("{dir}/node_modules is installed"))),
        None => {}
    }

    out
}

/// The program `sh -c` will resolve on `PATH` for a `beforeBuildCommand` — its first word, and
/// only when the line is a plain `program args…`.
///
/// `None` for anything else: an environment assignment (`FOO=1 pnpm build`), a pipeline, a
/// subshell, a `cd … && …`. This check earns its place only while it is certain, because a wrong
/// answer here is a *failure* that refuses to build something which would have worked — so the
/// shapes it cannot read become a warning that says so rather than a guess.
///
/// The syntax test is over the **whole line** and not just its first word, and that is the case
/// worth spelling out: `cd ui && pnpm build` has a perfectly ordinary first word, and `cd` is a
/// shell builtin that is nevertheless a real file at `/usr/bin/cd` on macOS and on no ordinary
/// Linux — so a first-word-only rule would refuse that config here and pass it there, for a
/// reason having nothing to do with the tool anybody cares about.
fn frontend_tool(script: &str) -> Option<&str> {
    let shell_syntax = |c: char| "|&;<>()$`\\\"'*?[]{}=".contains(c);
    if script.contains(shell_syntax) {
        return None;
    }
    script.split_whitespace().next()
}

/// Where the frontend build runs, spelled from the workspace root.
///
/// A path and not the config's own `../../ui`, because the reader of a verdict is standing at
/// the workspace root and not inside `crates/cide-app`. With no `cwd` in the config it is
/// tauri's own frontend directory, which this task does not compute — so it is named rather
/// than invented.
fn frontend_dir(build: &BeforeBuild) -> String {
    match build.cwd.as_deref() {
        Some(cwd) => normalize(&Path::new(APP_CRATE).join(cwd))
            .display()
            .to_string(),
        None => "tauri's frontend directory".into(),
    }
}

/// Resolve `..` and `.` textually, the way `Path::join` does not.
///
/// Used to answer "which file will the bundler open", which `canonicalize` cannot: the paths
/// this is asked about — the sidecar, the frontend directory of a checkout that may not be this
/// one — need not exist at the moment the question is asked.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
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

// --- the source tarball --------------------------------------------------------------------

/// How many file names a verdict lists before it says "and N more".
///
/// A dirty tree during a release is usually one or two files; a tree with sixty is one nobody
/// should be releasing from, and printing sixty paths would bury the sentence that says so.
const NAMED_FILES: usize = 5;

/// What `git` says about this checkout, gathered once so the verdicts over it can be pure.
///
/// Split the same way [`signing_verdicts`] is split, and for the same reason: a verdict that
/// shells out while it is deciding what to say can only be tested by fabricating a repository
/// on disk, and the wording — which is the whole value of these checks — would then be asserted
/// against whatever the fabrication happened to produce.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SrcStatus {
    /// Whether `git` is on `PATH` at all. Everything else below is unknown when it is not.
    git: bool,
    /// `git rev-parse --show-toplevel`. `None` means this is not a git checkout.
    toplevel: Option<String>,
    /// Whether that toplevel is the workspace root, rather than an enclosing repository.
    toplevel_is_root: bool,
    /// The abbreviated commit `HEAD` names. `None` means a repository with no commits.
    head: Option<String>,
    /// Tracked files that differ from `HEAD`, staged or not, capped at [`NAMED_FILES`].
    dirty: Vec<String>,
    dirty_total: usize,
    /// Files git has never been told about, capped the same way.
    untracked: Vec<String>,
    untracked_total: usize,
    /// Every tag pointing at `HEAD`, verbatim. Whether one of them *matches the version* is a
    /// judgement (is `v0.1.0` the same as `0.1.0`?) and belongs in `src_verdicts`, not here.
    tags: Vec<String>,
    /// `tar.tar.gz.command`, when this machine has configured one. The only thing that changes
    /// the tarball's bytes for a fixed commit.
    compressor: Option<String>,
}

/// Run `git` and judge what it said.
/// Whether the plan should actually cut a source tarball.
///
/// A free function taking the dirty count rather than a branch inside [`plan`], because [`plan`]
/// reads the real repository and a rule that can only be exercised against a live git checkout is
/// a rule no test can drive — which is exactly what happened: the verdict half of this decision
/// was asserted and the plan half was not, so removing the skip left every test green while a
/// warning printed a caution and the archive step produced the artefact the caution was about.
///
/// The pair it belongs to is in `src_verdicts`: a dirty tree is a `Fail` when the tarball was
/// named and a `Warn` when it was inherited. This is what makes the warning true.
///
/// `dirty_total == 0`, not `!dirty_total > 0` — `!` on a `usize` is bitwise NOT in Rust, so that
/// spelling compiles, reads as the negation, and is true for every value but `usize::MAX`.
fn archives_source(targets: Targets, dirty_total: usize) -> bool {
    targets.src && (targets.src_named || dirty_total == 0)
}

fn src_checks(root: &Path, info: &AppInfo, named: bool) -> Vec<Verdict> {
    src_verdicts(&read_src_status(root), info, named)
}

fn read_src_status(root: &Path) -> SrcStatus {
    let mut status = SrcStatus::default();
    if which("git").is_none() {
        return status;
    }
    status.git = true;

    // `success()` on purpose: every question below is asked in a form whose failure means "the
    // answer does not exist" (no repository, no commit, no configured key), which is exactly
    // what `None` says. Nothing here uses an exit code as data — `git diff --quiet` would, and
    // that is why the dirty check asks for names instead.
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let lines = |text: Option<String>| -> Vec<String> {
        text.into_iter()
            .flat_map(|t| {
                t.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    status.toplevel = git(&["rev-parse", "--show-toplevel"]);
    status.toplevel_is_root = status
        .toplevel
        .as_deref()
        .is_some_and(|top| same_dir(Path::new(top), root));
    status.head = git(&["rev-parse", "--short", "HEAD"]);

    if status.head.is_some() {
        // Names rather than `--quiet`, because the verdict has to say *which* files would be
        // left out; "the tree is dirty" sends a reader to `git status` to learn what this
        // command already knows. Against HEAD rather than against the index, so a staged-but-
        // uncommitted change counts — it is just as absent from the archive as an unstaged one.
        //
        // Measured in a throwaway repository, because "which of the four kinds of local change
        // does this see" is not a thing to assume. `git diff --name-only HEAD` reports an
        // unstaged edit, a deletion *and* a staged new file, and reports each once; an untracked
        // file appears only in `ls-files --others` below, so nothing is counted twice. All three
        // of the first kind are silent in the archive: the edited file goes in with its old
        // contents, the deleted one goes in, the staged-new one does not go in at all.
        let dirty = lines(git(&["diff", "--name-only", "HEAD"]));
        status.dirty_total = dirty.len();
        status.dirty = dirty.into_iter().take(NAMED_FILES).collect();

        status.tags = lines(git(&["tag", "--points-at", "HEAD"]));
    }

    let untracked = lines(git(&["ls-files", "--others", "--exclude-standard"]));
    status.untracked_total = untracked.len();
    status.untracked = untracked.into_iter().take(NAMED_FILES).collect();

    status.compressor = git(&["config", "--get", "tar.tar.gz.command"]).filter(|c| !c.is_empty());

    status
}

/// Whether two paths name the same directory, `..` and symlinks resolved.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `a, b and 3 more`, for a capped list.
fn name_some(named: &[String], total: usize) -> String {
    let mut text = named.join(", ");
    if total > named.len() {
        text.push_str(&format!(" and {} more", total - named.len()));
    }
    text
}

/// The verdicts, over a gathered [`SrcStatus`] rather than over a live `git`.
///
/// # The dirty-tree failure is the reason this function exists
///
/// A source tarball is a *claim about a commit*: it says "this is what cide 0.1.0 is", and its
/// checksum is supposed to be reproducible by anyone who checks out the tag. `git archive`
/// packages `HEAD` and nothing else, so a tree with uncommitted work produces a perfectly valid
/// tarball of something the developer is not looking at, with no warning anywhere. The mistake
/// surfaces weeks later as a bug report against code that was never released, or as a checksum
/// that matches nothing. So it is a `Fail`, it names the commit, and it names the files.
///
/// There is deliberately no `--allow-dirty`. A flag exists to be pasted into a script, and
/// there is no legitimate reason to publish a source tarball from a tree whose contents nobody
/// can reconstruct.
///
/// # …and untracked files are only a warning
///
/// They are just as absent from the archive, but they are usually scratch — a note, a profile
/// dump, a screenshot. The dangerous case is narrow: a *new source file* that committed code
/// already imports, which makes the tarball fail to build while the author's tree builds fine.
/// That is worth a sentence and not a refusal; this file's own rule is that a preflight people
/// learn to ignore is worse than one that says less.
fn src_verdicts(status: &SrcStatus, info: &AppInfo, named: bool) -> Vec<Verdict> {
    let mut out = Vec::new();

    if !status.git {
        out.push(Verdict::Fail(
            "git is not on PATH, and the source tarball is `git archive`: the tracked set at \
             HEAD is the only definition of \"the source\" this repository has, and a `tar` of \
             the working directory would ship target/, node_modules/ and every ignored file"
                .into(),
        ));
        return out;
    }

    let Some(toplevel) = status.toplevel.as_deref() else {
        out.push(Verdict::Fail(
            "this tree is not a git checkout, so there is nothing to archive. An unpacked \
             source tarball has no `.git` — it is a tree you build in, not one you cut a \
             release from"
                .into(),
        ));
        return out;
    };

    if !status.toplevel_is_root {
        out.push(Verdict::Fail(format!(
            "this workspace sits inside a larger git repository, whose root is {toplevel}. \
             `git archive HEAD` packages that project, not this one, and the result would look \
             plausible — right name, right version, wrong contents"
        )));
        return out;
    }

    let Some(head) = status.head.as_deref() else {
        out.push(Verdict::Fail(
            "HEAD names no commit, so `git archive` has nothing to package. A source tarball is \
             a claim about a commit; there is not one yet"
                .into(),
        ));
        return out;
    };

    out.push(Verdict::Ok(format!(
        "will archive {head} into {}, unpacking as {}",
        src_archive_path(info),
        src_prefix(info)
    )));

    if status.dirty_total > 0 {
        /*
         * A `Fail` only when the tarball was asked for by name.
         *
         * The hazard is the same either way — `git archive` packages HEAD and drops uncommitted
         * work silently, so the artefact claims a commit whose contents it does not have — but
         * who is standing in front of it is not. Someone typing `--src` is cutting a release and
         * wants to be stopped. Someone typing `package --run` is building an AppImage of the work
         * in progress, and `src` arrived in their target set from `Targets::LINUX` without being
         * mentioned; refusing to build anything at all, and printing advice to commit or stash
         * the very changes they are testing, answers a question they did not ask.
         *
         * So the default run warns and drops the archive step. Everything else still builds, and
         * the one artefact that would have been a lie is the one that is not produced.
         */
        let detail = format!(
            "the working tree has uncommitted changes to {} tracked file(s) ({}). `git archive` \
             packages HEAD ({head}) and would leave them out silently, so the tarball would \
             claim a commit whose contents it does not have",
            status.dirty_total,
            name_some(&status.dirty, status.dirty_total)
        );
        out.push(if named {
            Verdict::Fail(format!(
                "{detail}. Commit, stash, or check out the tag you mean to release"
            ))
        } else {
            Verdict::Warn(format!(
                "{detail}. The source tarball is skipped; every other target still builds. Ask \
                 for it by name with `--src` once the tree is clean"
            ))
        });
    }

    if status.untracked_total > 0 {
        out.push(Verdict::Warn(format!(
            "{} untracked file(s) will not be in the tarball ({}). Usually that is exactly \
             right; if one of them is a new source file the committed code already imports, the \
             tarball will not build and this tree will, so nothing else would notice",
            status.untracked_total,
            name_some(&status.untracked, status.untracked_total)
        )));
    }

    if let Some(compressor) = &status.compressor {
        out.push(Verdict::Warn(format!(
            "tar.tar.gz.command is configured (`{compressor}`), so the gzip layer is this \
             machine's compressor rather than git's built-in one: the same commit produces a \
             different file, and a different sha256, elsewhere. The tar layer inside is \
             identical either way — `gunzip -c | sha256sum` is the comparison that holds"
        )));
    }

    // `v0.1.0` and `0.1.0` are the same release; both spellings are in wide use, and a preflight
    // that accepted only one would warn about a correctly tagged tree half the time.
    if !status
        .tags
        .iter()
        .any(|tag| tag.trim_start_matches('v') == info.version)
    {
        // A warning and never a failure: `actions/checkout` fetches no tags by default, so this
        // fires on every CI run that has not asked for `fetch-depth: 0`, and a release cut from
        // a branch before the tag is pushed is a normal order of operations.
        out.push(Verdict::Warn(format!(
            "no tag at {head} matches {}, so the tarball's name comes from {TAURI_CONF} alone. \
             A release page pairs the artefact with a tag, and nothing here checks that they \
             agree (in CI this also fires whenever tags were not fetched)",
            info.version
        )));
    }

    out
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
pub fn generated(info: &AppInfo, ra: &RaLock, gopls: &RaLock) -> Vec<(String, String)> {
    vec![
        (manifest_path(info), flatpak_manifest(info, ra, gopls)),
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
    let ra = read_ra_lock(root)?;
    let gopls = read_gopls_lock(root)?;
    for (relative, contents) in generated(info, &ra, &gopls) {
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
    let ra = read_ra_lock(root)?;
    let gopls = read_gopls_lock(root)?;
    let mut stale = Vec::new();
    for (relative, expected) in generated(info, &ra, &gopls) {
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
    println!(
        "package: {} packaging files in sync",
        generated(info, &ra, &gopls).len()
    );
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
  <project_license>MIT</project_license>
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
/// Which key a Flatpak git source pins the fork with.
///
/// `commit:` for a 40-hex id, `tag:` for anything else. flatpak-builder refuses a tag name
/// under `commit:` and a commit id under `tag:`, so the manifest has to say which one the
/// lock holds — and the lock deliberately accepts both, because a fork pinned to an upstream
/// release tag and one pinned to a `cide`-branch commit are both legitimate states.
fn ra_source_key(rev: &str) -> &'static str {
    let is_commit = rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit());
    if is_commit { "commit" } else { "tag" }
}

pub fn flatpak_manifest(info: &AppInfo, ra: &RaLock, gopls: &RaLock) -> String {
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
  - org.freedesktop.Sdk.Extension.golang
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
      append-path: /usr/lib/sdk/rust-stable/bin:/usr/lib/sdk/node22/bin:/usr/lib/sdk/golang/bin
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
      # cide's rust-analyzer fork, from the pinned git source below. `env` on the one command
      # rather than build-options: those profile settings are upstream's own dist choices for
      # the fork, and putting them in the module environment would silently re-profile cide's
      # own release build above.
      - env CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CFG_RELEASE={ra_rev}+cide cargo build --release --locked --manifest-path rust-analyzer-fork/Cargo.toml -p rust-analyzer
      # cide's gopls, from its own pinned git source. Built in the checkout's gopls/ module
      # directory; CGO off because gopls is pure Go and static is simpler; the -X stamp is
      # logs-only, same rule as CFG_RELEASE above.
      - cd gopls-fork/gopls && env CGO_ENABLED=0 GOFLAGS=-trimpath go build -ldflags "-X main.version={gopls_rev}+cide" -o {gopls_bin} .
      - install -Dm755 target/release/cide /app/bin/{command}
      # cide-hook must sit beside the main binary: `cmd::session::hook_settings` locates it
      # relative to `current_exe`, because the child's cwd is the project root and its PATH
      # is the user's.
      - install -Dm755 target/release/cide-hook /app/bin/cide-hook
      - install -Dm755 target/release/cide-headless /app/bin/cide-headless
      # The fork sits beside the main binary too — `cide_core::toolchain::sibling_binary` —
      # and under its cide name, so it can never shadow a rust-analyzer of the user's own.
      - install -Dm755 rust-analyzer-fork/target/release/rust-analyzer /app/bin/{ra_bin}
      - install -Dm755 gopls-fork/gopls/{gopls_bin} /app/bin/{gopls_bin}
      - install -Dm644 crates/cide-app/icons/128x128.png /app/share/icons/hicolor/128x128/apps/{id}.png
      - install -Dm644 packaging/flatpak/{id}.desktop /app/share/applications/{id}.desktop
      - install -Dm644 packaging/flatpak/{id}.metainfo.xml /app/share/metainfo/{id}.metainfo.xml
    sources:
      - type: dir
        path: ../..
      # The fork, at exactly the revision packaging/rust-analyzer.lock pins. A git source
      # rather than a second dir source so a Flatpak build cannot silently package whatever
      # the sibling checkout happens to hold.
      - type: git
        url: {ra_url}
        {ra_key}: {ra_rev}
        dest: rust-analyzer-fork
      - type: git
        url: {gopls_url}
        {gopls_key}: {gopls_rev}
        dest: gopls-fork
"#,
        conf = TAURI_CONF,
        id = info.identifier,
        runtime = GNOME_RUNTIME,
        command = info.product_name,
        ra_bin = RA_BIN,
        ra_url = ra.url,
        ra_key = ra_source_key(&ra.rev),
        ra_rev = ra.rev,
        gopls_bin = GOPLS_BIN,
        gopls_url = gopls.url,
        gopls_key = ra_source_key(&gopls.rev),
        gopls_rev = gopls.rev,
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

    // Absent rather than malformed is not an error here: the preflight reports it, with the
    // sentence that says what the missing file costs. A hard error out of `read_app_info`
    // would also take out `--check` and `--write`, which do not need the overlay at all.
    let overlay_path = root.join(TAURI_BUNDLE_CONF);
    let overlay: serde_json::Value = fs::read_to_string(&overlay_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);

    // Same tolerance, same reason: a missing macOS overlay is a preflight verdict, not a hard
    // error out of a function that `--check` and `--write` also call.
    let macos: serde_json::Value = fs::read_to_string(root.join(TAURI_MACOS_CONF))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);

    Ok(AppInfo {
        identifier: string(conf.get("identifier"), "identifier")?,
        version: string(conf.get("version"), "version")?,
        product_name: string(conf.get("productName"), "productName")?,
        bundle_targets: list(conf.pointer("/bundle/targets")),
        icons: list(conf.pointer("/bundle/icon")),
        has_updater: conf.pointer("/plugins/updater").is_some(),
        external_bin: list(overlay.pointer("/bundle/externalBin")),
        appimage_files: overlay
            .pointer("/bundle/linux/appimage/files")
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        base_external_bin: list(conf.pointer("/bundle/externalBin")),
        macos_bundle_targets: list(macos.pointer("/bundle/targets")),
        macos_icons: list(macos.pointer("/bundle/icon")),
        macos_external_bin: list(macos.pointer("/bundle/externalBin")),
        before_build: read_before_build(conf.pointer("/build/beforeBuildCommand")),
    })
}

/// Read `build.beforeBuildCommand`, which tauri accepts in two shapes.
///
/// A bare string (`"pnpm build"`) runs in tauri's own frontend directory; the object form
/// (`{"script": …, "cwd": …}`) names where. Both are read although this repository uses only the
/// second, because a parser that understood one shape would answer `None` for the other — and
/// `None` is a verdict that says "nothing rebuilds the frontend", which would be the opposite of
/// the truth and would send a reader looking in the wrong place.
fn read_before_build(value: Option<&serde_json::Value>) -> Option<BeforeBuild> {
    let value = value?;
    if let Some(script) = value.as_str() {
        return Some(BeforeBuild {
            script: script.to_string(),
            cwd: None,
        });
    }
    Some(BeforeBuild {
        script: value.get("script")?.as_str()?.to_string(),
        cwd: value
            .get("cwd")
            .and_then(|cwd| cwd.as_str())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    /// The 32-bit-GTK trap, which cost a whole packaging run and reported only `failed to run
    /// linuxdeploy`. The shim must appear when — and only when — the host is in that state.
    #[test]
    fn a_gtk_shim_is_installed_only_when_the_unsuffixed_tool_is_the_wrong_class() {
        use super::{gtk_immodules_shim, is_elf64};
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("cide-gtk-shim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        let write = |name: &str, class: u8| {
            let path = dir.join(name);
            let mut f = std::fs::File::create(&path).expect("create");
            // Just the identification bytes: `is_elf64` reads byte 4 and nothing else.
            f.write_all(&[0x7f, b'E', b'L', b'F', class, 0, 0, 0])
                .expect("write");
            path
        };

        // Nothing there at all: the plugin's own fallback list reaches `-64` unaided.
        assert!(
            gtk_immodules_shim(&dir).is_none(),
            "an absent tool needs no shim"
        );

        // The healthy host: unsuffixed is already 64-bit.
        write("gtk-query-immodules-3.0", 2);
        write("gtk-query-immodules-3.0-64", 2);
        assert!(
            gtk_immodules_shim(&dir).is_none(),
            "a 64-bit tool must not put a directory on the front of PATH for nothing"
        );

        // The openSUSE multilib host this was found on.
        let wrong = write("gtk-query-immodules-3.0", 1);
        assert!(!is_elf64(&wrong), "the fixture is the 32-bit case");
        let (target, link) = gtk_immodules_shim(&dir).expect("this host needs the shim");
        assert_eq!(target, dir.join("gtk-query-immodules-3.0-64"));
        assert_eq!(
            link.file_name().and_then(|n| n.to_str()),
            Some("gtk-query-immodules-3.0"),
            "the link has to carry the name `command -v` looks up, or it is never found"
        );

        // And a host with no 64-bit tool to point at gets no dangling symlink.
        std::fs::remove_file(dir.join("gtk-query-immodules-3.0-64")).expect("remove");
        assert!(
            gtk_immodules_shim(&dir).is_none(),
            "a shim pointing at nothing would fail later and more confusingly"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A path with a quote in it must not end the shell string it is embedded in.
    #[test]
    fn shell_quoting_survives_a_quote() {
        use super::shell_quote;
        assert_eq!(shell_quote("/usr/bin/x"), "'/usr/bin/x'");
        assert_eq!(shell_quote("a'b"), r#"'a'\''b'"#);
    }

    use super::*;

    fn info() -> AppInfo {
        AppInfo {
            identifier: "dev.cide.ide".into(),
            version: "0.1.0".into(),
            product_name: "cide".into(),
            bundle_targets: vec!["appimage".into(), "deb".into()],
            appimage_files: Vec::new(),
            icons: vec!["icons/32x32.png".into()],
            has_updater: false,
            external_bin: vec!["../../target/release/cide-hook".into()],
            base_external_bin: Vec::new(),
            macos_bundle_targets: vec!["app".into(), "dmg".into()],
            macos_icons: vec!["icons/32x32.png".into()],
            macos_external_bin: Vec::new(),
            before_build: Some(before_build()),
        }
    }

    /// The checked-in `build.beforeBuildCommand`, spelled out here so the verdict tests do not
    /// depend on a file they are not about.
    fn before_build() -> BeforeBuild {
        BeforeBuild {
            script: "pnpm build".into(),
            cwd: Some("../../ui".into()),
        }
    }

    /// A fork pin, spelled out here rather than read from `packaging/rust-analyzer.lock` so
    /// the manifest tests assert the generator's shape, not the pin of the week.
    fn ra_lock() -> RaLock {
        RaLock {
            url: "https://github.com/rust-lang/rust-analyzer".into(),
            rev: "2026-08-24".into(),
        }
    }

    fn gopls_lock() -> RaLock {
        RaLock {
            url: "https://github.com/golang/tools".into(),
            rev: "gopls/v0.23.0".into(),
        }
    }

    #[test]
    fn the_checked_in_fork_lock_parses() {
        // The same claim `the_real_config_is_readable` makes about tauri.conf.json: the
        // generator restates this file's contents, so a malformed lock must fail here rather
        // than as a preflight surprise on somebody's release run.
        let lock = read_ra_lock(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
            .expect("packaging/rust-analyzer.lock");
        assert!(lock.url.starts_with("https://"), "{lock:?}");
        assert!(!lock.rev.is_empty());
    }

    #[test]
    fn the_checked_in_gopls_lock_parses() {
        let lock = read_gopls_lock(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
            .expect("packaging/gopls.lock");
        assert!(lock.url.starts_with("https://"), "{lock:?}");
        // The rev is a gopls release tag in golang/tools, whose tag namespace is prefixed.
        assert!(lock.rev.starts_with("gopls/"), "{lock:?}");
    }

    #[test]
    fn the_two_locks_use_distinct_key_names() {
        // release.yml sources both files into one shell; shared key names would have the
        // second `source` silently clobber the first pin.
        let text =
            "CIDE_RA_URL=https://a\nCIDE_RA_REV=1\nCIDE_GOPLS_URL=https://b\nCIDE_GOPLS_REV=2\n";
        let ra = parse_ra_lock(text).expect("ra keys");
        let gopls = parse_gopls_lock(text).expect("gopls keys");
        assert_eq!((ra.url.as_str(), ra.rev.as_str()), ("https://a", "1"));
        assert_eq!((gopls.url.as_str(), gopls.rev.as_str()), ("https://b", "2"));
    }

    #[test]
    fn the_gopls_build_is_static_stamped_and_lands_where_the_bundler_looks() {
        let steps = gopls_steps(Some(LINUX), Some("gopls/v0.23.0"));
        let build = &steps[0];
        assert_eq!(build.program, "go");
        assert_eq!(build.cwd, format!("{GOPLS_FORK_DIR}/gopls"));
        assert!(
            build.env.contains(&("CGO_ENABLED".into(), "0".into())),
            "{build:?}"
        );
        assert!(
            build
                .args
                .iter()
                .any(|a| a == "-X main.version=gopls/v0.23.0+cide"),
            "the stamp is logs-only but it should be there: {:?}",
            build.args
        );
        // And without a readable lock, the stamp is simply absent — an honest plan, not an
        // error (preflight is where the unreadable lock fails).
        let unstamped = gopls_steps(None, None);
        assert!(
            !unstamped[0].args.iter().any(|a| a.starts_with("-X")),
            "{unstamped:?}"
        );
        assert!(
            steps
                .iter()
                .any(|s| s.args.iter().any(|a| a == &gopls_sidecar_path(LINUX))),
            "{steps:?}"
        );
    }

    #[test]
    fn an_absent_go_fails_and_an_old_go_only_warns() {
        // Old-but-present is a warning because `GOTOOLCHAIN=auto` (the default since 1.21)
        // downloads the toolchain the module's `go` directive demands — the build still
        // works, it just needs the network once.
        assert!(matches!(
            go_toolchain_verdict(None, None, None),
            Verdict::Fail(_)
        ));
        let old = go_toolchain_verdict(
            Some(Path::new("/usr/bin/go")),
            Some("go version go1.25.5 linux/amd64"),
            Some("1.26.0"),
        );
        match old {
            Verdict::Warn(sentence) => {
                assert!(
                    sentence.contains("1.25.5") && sentence.contains("1.26.0"),
                    "{sentence}"
                );
            }
            other => panic!("expected a warning, got {other:?}"),
        }
        assert!(matches!(
            go_toolchain_verdict(
                Some(Path::new("/usr/bin/go")),
                Some("go version go1.26.1 linux/amd64"),
                Some("1.26.0"),
            ),
            Verdict::Ok(_)
        ));
    }

    #[test]
    fn the_lock_format_is_the_shell_sourceable_subset() {
        // release.yml sources this file; the parser reads the same bytes. Comments, blank
        // lines and unknown keys pass through, and a missing half is a refusal rather than a
        // half-pin.
        let lock = parse_ra_lock(
            "# comment\n\nCIDE_RA_URL=https://example.com/fork\nCIDE_RA_REV=2026-08-24\nFUTURE=x\n",
        )
        .expect("parses");
        assert_eq!(lock.url, "https://example.com/fork");
        assert_eq!(lock.rev, "2026-08-24");
        assert!(parse_ra_lock("CIDE_RA_URL=https://example.com/fork\n").is_none());
        assert!(parse_ra_lock("CIDE_RA_REV=\nCIDE_RA_URL=x\n").is_none());
    }

    #[test]
    fn a_tag_and_a_commit_pin_through_different_flatpak_keys() {
        // flatpak-builder refuses a tag under `commit:` and vice versa, so the generator has
        // to tell them apart — and a 39- or 41-character near-sha is a tag, not a commit.
        assert_eq!(ra_source_key("2026-08-24"), "tag");
        assert_eq!(
            ra_source_key("0123456789abcdef0123456789abcdef01234567"),
            "commit"
        );
        assert_eq!(
            ra_source_key("0123456789abcdef0123456789abcdef0123456"),
            "tag"
        );
    }

    #[test]
    fn the_manifest_builds_and_installs_the_fork_at_the_pin() {
        let manifest = flatpak_manifest(&info(), &ra_lock(), &gopls_lock());
        assert!(
            manifest.contains("/app/bin/cide-rust-analyzer"),
            "{manifest}"
        );
        assert!(manifest.contains("url: https://github.com/rust-lang/rust-analyzer"));
        // The fixture rev is a tag, so the source pins through `tag:`.
        assert!(manifest.contains("tag: 2026-08-24"));
        assert!(manifest.contains("dest: rust-analyzer-fork"));
        assert!(manifest.contains("url: https://github.com/golang/tools"));
        assert!(manifest.contains("dest: gopls-fork"));
        assert!(manifest.contains("org.freedesktop.Sdk.Extension.golang"));
    }

    #[test]
    fn the_tarball_carries_the_fork_beside_the_other_binaries() {
        let steps = tarball_steps(&info(), LINUX);
        let installs_fork = steps.iter().any(|s| {
            s.program == "install" && s.args.iter().any(|a| a.ends_with(&format!("bin/{RA_BIN}")))
        });
        assert!(installs_fork, "{steps:?}");
    }

    #[test]
    fn the_tarball_carries_the_gopls_build_too() {
        let steps = tarball_steps(&info(), LINUX);
        let installs = steps.iter().any(|s| {
            s.program == "install"
                && s.args
                    .iter()
                    .any(|a| a.ends_with(&format!("bin/{GOPLS_BIN}")))
        });
        assert!(installs, "{steps:?}");
    }

    #[test]
    fn a_bundler_plan_builds_the_gopls_sidecar_before_the_bundler_runs() {
        let steps = plan(Path::new("/nonexistent"), &info(), Targets::LINUX, LINUX);
        let copy = steps
            .iter()
            .position(|s| s.args.iter().any(|a| a == &gopls_sidecar_path(LINUX)));
        let bundler = steps
            .iter()
            .position(|s| s.args.first().is_some_and(|a| a == "tauri"));
        match (copy, bundler) {
            (Some(copy), Some(bundler)) => assert!(copy < bundler, "{steps:?}"),
            _ => panic!("the plan is missing the gopls copy or the bundler: {steps:?}"),
        }
    }

    #[test]
    fn a_bundler_plan_builds_the_fork_before_the_bundler_runs() {
        // The bundler resolves externalBin at its own start; a fork step after it is a fork
        // the bundle silently does not carry.
        let steps = plan(Path::new("/nonexistent"), &info(), Targets::LINUX, LINUX);
        let fork_copy = steps
            .iter()
            .position(|s| s.args.iter().any(|a| a == &ra_sidecar_path(LINUX)));
        let bundler = steps
            .iter()
            .position(|s| s.args.first().is_some_and(|a| a == "tauri"));
        match (fork_copy, bundler) {
            (Some(fork_copy), Some(bundler)) => {
                assert!(fork_copy < bundler, "{steps:?}");
            }
            _ => panic!("the plan is missing the fork copy or the bundler: {steps:?}"),
        }
    }

    /// The two hosts, spelled as `rustc -vV` spells them.
    ///
    /// Real triples rather than invented ones: `is_macos_triple` is a substring test, and a
    /// fixture that only *resembles* a triple would let a wrong substring through. Linux is what
    /// every gate in this repository runs on, so it is the default in the helpers below.
    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const MACOS: &str = "aarch64-apple-darwin";

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
        // `externalBin` is resolved relative to the crate holding tauri.conf.json and the
        // triple is appended; the plan writes its copy relative to the workspace root. The two
        // spellings have to name one file.
        //
        // The predecessor of this test asserted `resolved.ends_with("target/release/cide-hook")`
        // — which a config entry of plain `target/release/cide-hook`, missing the `../../` and
        // therefore pointing at a `crates/cide-app/target/` that never exists, satisfies. It
        // was green for the exact drift it was written to catch. Compare whole paths.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("tauri.conf.json parses");
        let triple = "x86_64-unknown-linux-gnu";
        let entry = info
            .external_bin
            .iter()
            .find(|p| p.ends_with(HOOK_BIN))
            .expect("an externalBin entry for the hook");

        let bundler_opens = normalize(&root.join(APP_CRATE).join(format!("{entry}-{triple}")));
        let plan_writes = root.join(sidecar_path(triple));
        assert_eq!(
            bundler_opens,
            plan_writes,
            "the bundler will open {}, but the plan writes {}",
            bundler_opens.display(),
            plan_writes.display()
        );
    }

    #[test]
    fn the_base_config_must_not_carry_the_sidecar() {
        // `bundle.externalBin` is not bundler-only: `tauri-build` copies every entry on every
        // cargo invocation and errors when one is missing. In tauri.conf.json it therefore
        // stops `cargo build --workspace`, `cargo test --workspace` and CI dead on any tree
        // without a prebuilt release cide-hook — which is every fresh clone. It belongs in the
        // overlay that only `cargo tauri build --config` reads.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("tauri.conf.json parses");
        assert!(
            info.base_external_bin.is_empty(),
            "{TAURI_CONF} must not carry bundle.externalBin (found {:?}); \
             it belongs in {TAURI_BUNDLE_CONF}",
            info.base_external_bin
        );
    }

    #[test]
    fn a_sidecar_in_the_base_config_is_a_preflight_failure() {
        let mut info = info();
        info.base_external_bin = vec!["../../target/release/cide-hook".into()];
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::LINUX, LINUX);
        assert!(
            checks
                .iter()
                .any(|c| matches!(c, Verdict::Fail(d) if d.contains("every `cargo build`"))),
            "{checks:?}"
        );
    }

    #[test]
    fn the_bundler_step_reads_the_overlay_that_holds_the_sidecar() {
        // Without `--config`, tauri-cli sees only tauri.conf.json — which no longer names the
        // hook — and produces a bundle with no cide-hook and no error. This flag is the only
        // thing connecting the overlay to the build.
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            Targets::LINUX,
            "x86_64-unknown-linux-gnu",
        );
        let bundle = steps
            .iter()
            .find(|s| s.args.first().map(String::as_str) == Some("tauri"))
            .expect("the bundler step");
        let config = bundle
            .args
            .iter()
            .position(|a| a == "--config")
            .and_then(|i| bundle.args.get(i + 1))
            .expect("a --config flag naming the overlay");
        // The step runs in APP_CRATE and `--config` resolves against the process cwd, so the
        // flag's value joined onto the cwd has to be the checked-in file.
        assert_eq!(
            Path::new(APP_CRATE).join(config),
            Path::new(TAURI_BUNDLE_CONF)
        );
        let root = crate::workspace_root().expect("a workspace root");
        assert!(
            root.join(TAURI_BUNDLE_CONF).exists(),
            "{TAURI_BUNDLE_CONF} is named by the plan but is not checked in"
        );
    }

    #[test]
    fn a_missing_sidecar_entry_is_a_failure_not_a_warning() {
        let mut info = info();
        info.external_bin.clear();
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::LINUX, LINUX);
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
            Targets::LINUX,
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
            app: false,
            dmg: false,
            tarball: false,
            src: false,
            src_named: false,
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
            Targets::LINUX,
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
        // By key, not by position, and the set is checked rather than the length. A second
        // variable appears here on a host that needs the `gtk-query-immodules-3.0` shim, and
        // whether this machine is one of those is not something the test can decide — so the
        // assertion has to tolerate that entry by name while still failing on any other.
        let runtime = handed
            .iter()
            .find(|(k, _)| k == LDAI_RUNTIME_FILE)
            .unwrap_or_else(|| panic!("the runtime must reach the bundler: {handed:?}"));
        assert!(
            runtime.1.starts_with('/'),
            "appimagetool runs with its own cwd, so this has to be absolute: {handed:?}"
        );
        for (key, value) in handed {
            assert!(
                key == LDAI_RUNTIME_FILE || key == "PATH",
                "unexpected variable handed to the bundler: {key}={value}"
            );
        }
        if let Some((_, path)) = handed.iter().find(|(k, _)| k == "PATH") {
            assert!(
                path.starts_with('/') && path.contains(GTK_SHIM_DIR),
                "the only PATH cide sets here is the shim, prepended and absolute: {path}"
            );
        }
    }

    #[test]
    fn an_already_fetched_runtime_is_not_fetched_again() {
        let dir = std::env::temp_dir().join(format!("cide-xtask-{}", std::process::id()));
        let runtime = dir.join(appimage_runtime_path("x86_64-unknown-linux-gnu"));
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::write(&runtime, b"not really a runtime").unwrap();

        let steps = plan(&dir, &info(), Targets::LINUX, "x86_64-unknown-linux-gnu");
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
            app: false,
            dmg: false,
            tarball: false,
            src: false,
            src_named: false,
        };
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            targets,
            "x86_64-unknown-linux-gnu",
        );
        assert!(steps.iter().all(|s| s.program != "curl"), "{steps:?}");
        // Not `env.is_empty()` any more: the fork build step legitimately carries its two
        // CARGO_PROFILE_* settings. What a deb-only plan must not carry is the AppImage
        // runtime override, which is what this test was always about.
        assert!(
            steps
                .iter()
                .all(|s| s.env.iter().all(|(k, _)| k != LDAI_RUNTIME_FILE)),
            "{steps:?}"
        );
    }

    #[test]
    fn the_webkit_helper_check_fails_when_nothing_maps_the_helpers() {
        // This test used to assert `never a hard failure`, and that assertion was the bug it
        // should have caught: an AppImage without these helpers does not degrade on an unusual
        // host, it aborts before its first frame on every host including the one that built it.
        // A warning let a package ship that could not start.
        //
        // Not a fixture for the machine half — the point of the check is to describe the
        // machine it runs on, and a mocked filesystem would only assert the mock was read.
        let Some(_) = find_webkit_helpers_elsewhere() else {
            // The bundler's own search will find them; nothing to map and nothing to assert.
            return;
        };

        let bare = info();
        assert!(bare.appimage_files.is_empty(), "fixture starts unmapped");
        let verdict = webkit_helper_check(&bare);
        assert!(
            matches!(verdict, Verdict::Fail(_)),
            "an unmapped bundle must fail, not warn: {verdict:?}"
        );
        // Both helpers named, not just one: the bundler's loop is per-file, so a machine
        // holding one of the two must still be told which is missing.
        if let Verdict::Fail(detail) = &verdict {
            assert!(detail.contains("WebKitWebProcess"), "{detail}");
            assert!(detail.contains("WebKitNetworkProcess"), "{detail}");
        }
    }

    #[test]
    fn the_shipped_config_maps_them() {
        // The real file, because that is the artefact that decides whether a package runs.
        // `workspace_root` lives in main.rs and is not visible here; the manifest dir of this
        // crate is its child, which is the same answer by a route a test can take.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a parent")
            .to_path_buf();
        let Ok(real) = read_app_info(&root) else {
            return;
        };
        if find_webkit_helpers_elsewhere().is_none() {
            return;
        }
        assert!(
            matches!(webkit_helper_check(&real), Verdict::Ok(_)),
            "the checked-in bundle config must map WebKit's helpers, or the AppImage cannot \
             start: {:?}",
            webkit_helper_check(&real)
        );
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
            optional: false,
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

    // --- the frontend build, which is the bundler's first step -------------------------------

    #[test]
    fn a_missing_frontend_tool_is_a_failure_that_says_what_the_build_will_print() {
        // Observed on the first Mac to run `./build.sh`: `sh: pnpm: command not found`, exit code
        // 127, after `cargo build --release -p cide-hook` had already run. Every fact needed to
        // say so was available before the first compile, which is what this verdict is.
        let checks = frontend_verdicts(&before_build(), None, Some(true));
        let refusal = checks
            .iter()
            .find(|c| matches!(c, Verdict::Fail(d) if d.contains("pnpm")))
            .unwrap_or_else(|| panic!("no missing-tool refusal in {checks:?}"));
        let detail = refusal.detail();
        assert!(
            detail.contains("127") && detail.contains("command not found"),
            "the refusal has to match what the build would print, or a reader cannot connect \
             the two: {detail}"
        );
        assert!(
            detail.contains("corepack") || detail.contains("brew"),
            "and say how to fix it, since this is the one prerequisite a Mac is likely to be \
             missing: {detail}"
        );
    }

    #[test]
    fn uninstalled_frontend_dependencies_are_the_other_half_of_the_same_round_trip() {
        // A fresh clone is missing both, and reporting only the tool would send somebody back for
        // a second twenty-minute failure. Both verdicts come out of one pass.
        let checks = frontend_verdicts(&before_build(), None, Some(false));
        assert_eq!(
            checks
                .iter()
                .filter(|c| matches!(c, Verdict::Fail(_)))
                .count(),
            2,
            "{checks:?}"
        );
        assert!(
            checks.iter().any(|c| matches!(c, Verdict::Fail(d)
                if d.contains("ui/node_modules") && d.contains("pnpm --dir ui install"))),
            "the dependency failure names the directory and the command: {checks:?}"
        );
    }

    #[test]
    fn an_installed_toolchain_says_where_it_found_it() {
        let checks = frontend_verdicts(&before_build(), Some("/usr/bin/pnpm".into()), Some(true));
        assert!(
            checks.iter().all(|c| matches!(c, Verdict::Ok(_))),
            "{checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.detail().contains("/usr/bin/pnpm") && c.detail().contains("pnpm build")),
            "{checks:?}"
        );
    }

    #[test]
    fn a_command_that_is_not_a_plain_program_is_warned_about_rather_than_guessed_at() {
        // A wrong answer here refuses a build that would have worked, so the shapes this cannot
        // read say so instead of failing on a first word that is not a program.
        assert_eq!(frontend_tool("pnpm build"), Some("pnpm"));
        assert_eq!(frontend_tool("npm run build"), Some("npm"));
        assert_eq!(frontend_tool("VITE_FOO=1 pnpm build"), None);
        // Not `Some("cd")`: `cd` is a builtin here and a real /usr/bin/cd on a Mac, so a
        // first-word-only rule would refuse this config on one platform and pass it on the other.
        assert_eq!(frontend_tool("cd ui && pnpm build"), None);
        assert_eq!(frontend_tool("(pnpm build)"), None);
        assert_eq!(frontend_tool(""), None);

        let odd = BeforeBuild {
            script: "VITE_FOO=1 pnpm build".into(),
            cwd: Some("../../ui".into()),
        };
        let checks = frontend_verdicts(&odd, None, Some(true));
        assert!(
            checks.iter().all(|c| !matches!(c, Verdict::Fail(_)))
                && checks.iter().any(|c| matches!(c, Verdict::Warn(_))),
            "{checks:?}"
        );
    }

    #[test]
    fn the_verdicts_name_the_directory_from_the_workspace_root() {
        // `../../ui` is right only if you are standing in crates/cide-app, and a reader of a
        // preflight is standing at the root.
        assert_eq!(frontend_dir(&before_build()), "ui");
        assert_eq!(
            frontend_dir(&BeforeBuild {
                script: "pnpm build".into(),
                cwd: None
            }),
            "tauri's frontend directory"
        );
    }

    #[test]
    fn the_real_config_still_names_a_frontend_build() {
        // The whole check is driven by the config rather than by a hardcoded "pnpm", so that
        // renaming the command in `tauri.conf.json` moves the verdict with it instead of quietly
        // checking a tool nothing runs.
        let root = crate::workspace_root().expect("a workspace root");
        let info = read_app_info(&root).expect("the checked-in config is readable");
        let build = info
            .before_build
            .expect("tauri.conf.json configures beforeBuildCommand");
        assert_eq!(frontend_tool(&build.script), Some("pnpm"));
        assert_eq!(frontend_dir(&build), "ui");
    }

    #[test]
    fn both_shapes_of_before_build_command_are_understood() {
        // tauri accepts a bare string as well as the object form, and a parser that read only one
        // would report "nothing rebuilds the frontend" for a config that does.
        let string = read_before_build(Some(&serde_json::json!("pnpm build")));
        assert_eq!(
            string,
            Some(BeforeBuild {
                script: "pnpm build".into(),
                cwd: None
            })
        );
        let object = read_before_build(Some(
            &serde_json::json!({"script": "pnpm build", "cwd": "../../ui"}),
        ));
        assert_eq!(object, Some(before_build()));
        assert_eq!(read_before_build(None), None);
        assert_eq!(read_before_build(Some(&serde_json::json!({}))), None);
    }

    #[test]
    fn no_frontend_build_at_all_is_a_warning_and_never_a_failure() {
        // A config with no hook means somebody builds the frontend by hand, which is a legitimate
        // arrangement — but nothing in the plan then refreshes `ui/dist`, and a bundle carrying
        // last week's frontend is the failure worth naming.
        let mut info = info();
        info.before_build = None;
        let checks = frontend_checks(Path::new("/nonexistent"), &info);
        assert!(
            checks.len() == 1 && matches!(&checks[0], Verdict::Warn(d) if d.contains("ui/dist")),
            "{checks:?}"
        );
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
        assert_eq!(
            flatpak_manifest(&info(), &ra_lock(), &gopls_lock()),
            flatpak_manifest(&info(), &ra_lock(), &gopls_lock())
        );
    }

    #[test]
    fn the_manifest_names_the_real_binaries_and_id() {
        let manifest = flatpak_manifest(&info(), &ra_lock(), &gopls_lock());
        assert!(manifest.contains("app-id: dev.cide.ide"));
        assert!(manifest.contains("/app/bin/cide-hook"));
        assert!(manifest.contains("command: cide"));
    }

    #[test]
    fn the_manifest_vendors_rather_than_taking_libgit2_from_the_runtime() {
        // Adding a libgit2 or openssl module would silently take precedence over the
        // vendored build and tie the package to the runtime's ABI.
        let manifest = flatpak_manifest(&info(), &ra_lock(), &gopls_lock());
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
        let manifest = flatpak_manifest(&info(), &ra_lock(), &gopls_lock());
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
        let manifest = flatpak_manifest(&info(), &ra_lock(), &gopls_lock());
        assert!(manifest.contains("--filesystem=host"));
        assert!(manifest.contains("--talk-name=org.freedesktop.Flatpak"));
    }

    #[test]
    fn the_tauri_bundle_list_is_one_command() {
        let steps = plan(
            Path::new("/nonexistent"),
            &info(),
            Targets::LINUX,
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
            app: false,
            dmg: false,
            tarball: false,
            src: false,
            src_named: false,
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
        let paths: Vec<String> = generated(&info(), &ra_lock(), &gopls_lock())
            .into_iter()
            .map(|(p, _)| p)
            .collect();
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
        let manifest = flatpak_manifest(&info, &ra_lock(), &gopls_lock());
        for (path, _) in generated(&info, &ra_lock(), &gopls_lock()) {
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
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::LINUX, LINUX);
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
        let checks = preflight(Path::new("/nonexistent"), &info(), Targets::LINUX, LINUX);
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
            optional: false,
        };
        assert_eq!(step.display(), "cd crates/cide-app && cargo tauri build");
    }

    // --- macOS -------------------------------------------------------------------------------
    //
    // Nothing below has ever been observed. It is a set of assertions about *configuration*,
    // written from `tauri-utils`' schema and `tauri-bundler`'s macOS bundler, and its whole job
    // is to make the first real Mac build fail for reasons that are about the Mac rather than
    // about a config nobody could check from here. Both branches are reachable on this machine
    // only because the host is a parameter; see `Targets::for_host`.

    #[test]
    fn a_target_the_host_cannot_build_is_refused_with_the_reason_in_it() {
        // The point of this verdict is that it arrives in the first second rather than twenty
        // minutes later inside `codesign`, and that it says why rather than "unsupported".
        let checks = preflight(Path::new("/nonexistent"), &info(), Targets::MACOS, LINUX);
        let refusal = checks
            .iter()
            .find(|c| matches!(c, Verdict::Fail(d) if d.contains("Cross-compilation")))
            .unwrap_or_else(|| panic!("no cross-compilation refusal in {checks:?}"));
        let detail = refusal.detail();
        assert!(
            detail.contains("app, dmg") && detail.contains(LINUX),
            "the refusal must name both the targets and the host: {detail}"
        );
        assert!(
            detail.contains("SDK") || detail.contains("licensed"),
            "and why it cannot simply be cross-compiled, or a reader tries `--target`: {detail}"
        );
    }

    #[test]
    fn the_refusal_runs_the_other_way_too() {
        // Symmetric on purpose. A Mac cannot produce an AppImage either, and a macOS CI runner
        // that ran the default `cargo xtask package` would otherwise get a plan it cannot run.
        let checks = preflight(Path::new("/nonexistent"), &info(), Targets::LINUX, MACOS);
        assert!(
            checks
                .iter()
                .any(|c| matches!(c, Verdict::Fail(d) if d.contains("can only be built on Linux"))),
            "{checks:?}"
        );
    }

    #[test]
    fn naming_no_target_means_what_this_host_can_build() {
        assert_eq!(Targets::for_host(LINUX), Targets::LINUX);
        assert_eq!(Targets::for_host(MACOS), Targets::MACOS);
        // …and the two do not overlap, so the default can never trip the refusal above.
        assert!(Targets::for_host(LINUX).impossible_on(LINUX).is_empty());
        assert!(Targets::for_host(MACOS).impossible_on(MACOS).is_empty());
    }

    #[test]
    fn an_ios_triple_is_not_a_mac() {
        // `is_macos_triple` is a substring test, and `apple` alone would match a target that is
        // neither a host nor anything this task can build for.
        assert!(is_macos_triple("aarch64-apple-darwin"));
        assert!(is_macos_triple("x86_64-apple-darwin"));
        assert!(!is_macos_triple("aarch64-apple-ios"));
        assert!(!is_macos_triple(LINUX));
    }

    #[test]
    fn the_dmg_is_asked_for_from_the_overlay_and_not_from_the_base_config() {
        // The overlay *replaces* `bundle.targets` rather than extending it, so the base config
        // is never going to mention `dmg` and asking it would produce a second, misleading
        // failure beside the honest cross-compilation one.
        let checks = preflight(Path::new("/nonexistent"), &info(), Targets::MACOS, MACOS);
        assert!(
            checks.iter().any(
                |c| matches!(c, Verdict::Ok(d) if d.contains(TAURI_MACOS_CONF)
                                                    && d.contains("bundles `dmg`"))
            ),
            "{checks:?}"
        );
        assert!(
            !checks.iter().any(|c| matches!(c, Verdict::Fail(d)
                if d.contains(TAURI_CONF) && d.contains("dmg"))),
            "the base config must not be blamed for a target only the overlay can declare: \
             {checks:?}"
        );
    }

    #[test]
    fn an_unsigned_mac_build_is_a_warning_and_says_what_a_user_will_see() {
        // Not a failure: an unsigned local build is the only thing available without a paid
        // Apple Developer Program membership, and it is exactly what a first Mac build wants.
        // The wording matters as much as the level — Gatekeeper's message for a quarantined
        // unsigned bundle blames the *download* ("is damaged"), so a reader who has not been
        // told will go looking for a corrupt file.
        let unsigned = signing_verdicts(&[], &MACOS_NOTARY_VARS);
        let warning = unsigned
            .iter()
            .find(|c| matches!(c, Verdict::Warn(d) if d.contains("unsigned")))
            .unwrap_or_else(|| panic!("no unsigned-bundle warning in {unsigned:?}"));
        assert!(
            warning.detail().contains("damaged") && warning.detail().contains("quarantine"),
            "the warning has to name what the user sees: {}",
            warning.detail()
        );
        // …and nothing about notarisation, which is noise on a build that has no signature to
        // notarise in the first place.
        assert_eq!(unsigned.len(), 1, "{unsigned:?}");

        // With a certificate but no notary credentials, the second warning is the useful one.
        let signed = signing_verdicts(&["APPLE_SIGNING_IDENTITY"], &["APPLE_ID"]);
        assert!(
            matches!(signed.first(), Some(Verdict::Ok(_))),
            "a configured signing identity is not a warning: {signed:?}"
        );
        assert!(
            signed
                .iter()
                .any(|c| matches!(c, Verdict::Warn(d) if d.contains("not be notarised"))),
            "{signed:?}"
        );

        // Fully configured: no warnings at all, or the preflight cries wolf on the one setup
        // that is actually correct.
        let full = signing_verdicts(&["APPLE_SIGNING_IDENTITY"], &[]);
        assert!(full.iter().all(|c| matches!(c, Verdict::Ok(_))), "{full:?}");
    }

    #[test]
    fn the_macos_overlay_must_not_carry_the_sidecar() {
        // The mirror of `a_sidecar_in_the_base_config_is_a_preflight_failure`, one platform
        // over and invisible from here: `tauri-build` reads the *host's* platform overlay on
        // every cargo invocation, so this key in that file breaks `cargo build --workspace` on
        // a Mac and nowhere else. Asserted against the checked-in file as well as against a
        // fixture, because the checked-in file is the one that can break somebody's build.
        let root = crate::workspace_root().expect("a workspace root");
        let live = read_app_info(&root).expect("tauri.macos.conf.json parses");
        assert!(
            live.macos_external_bin.is_empty(),
            "{TAURI_MACOS_CONF} carries bundle.externalBin ({:?}); that breaks `cargo build \
             --workspace` on macOS and on no other platform",
            live.macos_external_bin
        );

        let mut info = info();
        info.macos_external_bin = vec!["../../target/release/cide-hook".into()];
        let checks = preflight(Path::new("/nonexistent"), &info, Targets::MACOS, MACOS);
        assert!(
            checks.iter().any(|c| matches!(c, Verdict::Fail(d)
                if d.contains(TAURI_MACOS_CONF) && d.contains("externalBin"))),
            "{checks:?}"
        );
    }

    #[test]
    fn the_checked_in_overlay_declares_what_a_mac_can_actually_build() {
        // Against the real file. Without this the overlay could be deleted or emptied and every
        // gate here would stay green while a Mac build produced an `appimage` request and then
        // no artefacts at all.
        let root = crate::workspace_root().expect("a workspace root");
        let live = read_app_info(&root).expect("tauri.macos.conf.json parses");
        assert_eq!(
            live.macos_bundle_targets,
            vec!["app".to_string(), "dmg".to_string()],
            "{TAURI_MACOS_CONF} must override bundle.targets: the base config asks for appimage \
             and deb, which a Mac cannot build, and the merge replaces the array rather than \
             extending it"
        );
        assert!(
            !live.macos_icons.is_empty(),
            "{TAURI_MACOS_CONF} must carry its own bundle.icon, or the replaced-array merge \
             leaves the Mac build with the base list — including 48x48, which is not an ICNS \
             size and is resized to 32 for nothing"
        );
        for icon in &live.macos_icons {
            assert!(
                root.join(APP_CRATE).join(icon).exists(),
                "{icon} is named in {TAURI_MACOS_CONF} but is not on disk"
            );
        }
    }

    #[test]
    fn the_mac_plan_builds_the_sidecar_and_fetches_no_appimage_runtime() {
        // The `.app` must carry `cide-hook` for the same reason the AppImage must — a bundle
        // without it launches and every session in it runs with no hooks — and on macOS the
        // sidecar lands in `Contents/MacOS/`, which is exactly where `current_exe().parent()`
        // looks. The AppImage runtime fetch is a 3 MB download that would be pure waste here.
        let steps = plan(Path::new("/nonexistent"), &info(), Targets::MACOS, MACOS);
        let printed: Vec<String> = steps.iter().map(Step::display).collect();
        assert!(
            printed.iter().any(|s| s.contains("-p cide-hook")),
            "the mac plan does not build the hook sidecar: {printed:?}"
        );
        assert!(
            printed.iter().any(|s| s.contains("--bundles app,dmg")),
            "{printed:?}"
        );
        assert!(
            !printed.iter().any(|s| s.contains("appimage-runtime")),
            "the mac plan fetches the AppImage runtime: {printed:?}"
        );
        assert!(
            !printed.iter().any(|s| s.contains("flatpak-builder")),
            "the mac plan runs flatpak-builder: {printed:?}"
        );
    }

    // --- the source tarball ------------------------------------------------------------------

    /// A checkout with nothing wrong with it, as `read_src_status` would report one.
    fn clean() -> SrcStatus {
        SrcStatus {
            git: true,
            toplevel: Some("/home/dev/cide".into()),
            toplevel_is_root: true,
            head: Some("144260a".into()),
            dirty: Vec::new(),
            dirty_total: 0,
            untracked: Vec::new(),
            untracked_total: 0,
            tags: vec!["v0.1.0".into()],
            compressor: None,
        }
    }

    /// A `Targets` naming only the binary tarball.
    fn tarball_only() -> Targets {
        Targets {
            appimage: false,
            deb: false,
            flatpak: false,
            app: false,
            dmg: false,
            tarball: true,
            src: false,
            src_named: false,
        }
    }

    #[test]
    fn the_binary_tarball_is_linux_only_and_is_not_a_bundler_format() {
        // The contrast with `--src` below is the point. That one is `git archive` and runs
        // anywhere; this one archives ELF binaries linked against the host's WebKitGTK, so a Mac
        // asking for it is asking for a cross-compilation this repository does not do.
        assert!(tarball_only().impossible_on(LINUX).is_empty());
        assert_eq!(tarball_only().impossible_on(MACOS), vec!["tarball"]);
        // Not in `--bundles`: `cargo tauri build` has no format for it, and passing the name
        // through would fail inside the bundler with an unknown-target error.
        assert_eq!(tarball_only().bundles(), None);
    }

    #[test]
    fn the_binary_tarball_is_in_the_linux_default_set_only() {
        const { assert!(Targets::LINUX.tarball) };
        const { assert!(!Targets::MACOS.tarball) };
        assert!(Targets::for_host(LINUX).tarball);
        assert!(!Targets::for_host(MACOS).tarball);
    }

    #[test]
    fn the_tarball_is_named_for_the_version_and_the_triples_architecture() {
        let info = info();
        assert_eq!(
            tarball_name(&info, LINUX),
            format!("{}-{}-linux-x86_64.tar.gz", info.product_name, info.version)
        );
        assert_eq!(
            tarball_name(&info, "aarch64-unknown-linux-gnu"),
            format!(
                "{}-{}-linux-aarch64.tar.gz",
                info.product_name, info.version
            ),
            "the architecture is the triple's, not this machine's"
        );
        // Under the directory `artefacts` enumerates, or the release script that globs that tree
        // never sees it. `is_artefact` already answers yes for `*.tar.gz`.
        assert!(tarball_path(&info, LINUX).starts_with(TARBALL_BUNDLE_DIR));
        assert!(is_artefact(&tarball_name(&info, LINUX)));
    }

    #[test]
    fn the_tarball_prefix_has_no_trailing_slash_and_the_source_prefix_does() {
        // Two archives, two mechanisms. `git archive --prefix` is string concatenation onto
        // every path, so it needs the slash; `tar -C <stage> <prefix>` names a real directory,
        // and a trailing slash there is at best noise. Asserted together because the pair is
        // easy to "tidy" into agreement, and one of them would then be wrong.
        let info = info();
        assert!(src_prefix(&info).ends_with('/'));
        assert!(!tarball_prefix(&info).ends_with('/'));
        assert_eq!(
            src_prefix(&info).trim_end_matches('/'),
            tarball_prefix(&info)
        );
    }

    #[test]
    fn a_tarball_only_plan_builds_the_frontend_and_both_binaries() {
        let steps = plan(Path::new("/nonexistent"), &info(), tarball_only(), LINUX);
        let printed: Vec<String> = steps.iter().map(Step::display).collect();
        let joined = printed.join("\n");

        // Nothing else in this plan runs `beforeBuildCommand`, so it has to appear here or the
        // binary embeds whatever `ui/dist` happened to hold.
        assert!(
            joined.contains(r#"sh -c "pnpm build""#),
            "the frontend is built: {joined}"
        );
        assert!(
            joined.contains("cargo build --release --locked -p cide-app -p cide-hook"),
            "both binaries are built: {joined}"
        );
        // The bundler-only sidecar copy has no business in a plan with no bundler in it.
        assert!(
            !joined.contains(&sidecar_path(LINUX)),
            "no `cide-hook-<triple>` sidecar: {joined}"
        );
        assert!(
            printed
                .last()
                .is_some_and(|last: &String| last.starts_with("tar ")),
            "the archive is last: {printed:?}"
        );
    }

    #[test]
    fn a_bundled_plan_does_not_build_the_frontend_twice() {
        // The whole reason the tarball's build steps are conditional. `cargo tauri build` runs
        // `beforeBuildCommand` itself and compiles `cide-app` on the way to the AppImage; adding
        // the steps unconditionally would print a plan that claims two frontend builds and two
        // compilations, and a reader pasting it would perform them.
        let mut targets = tarball_only();
        targets.appimage = true;
        let joined = plan(Path::new("/nonexistent"), &info(), targets, LINUX)
            .iter()
            .map(|s| s.display())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            joined.matches("pnpm build").count(),
            0,
            "the bundler's beforeBuildCommand is the only frontend build: {joined}"
        );
        assert!(
            !joined.contains("-p cide-app"),
            "the bundler compiles the app crate itself: {joined}"
        );
        // ...but the tarball is still cut, from what the bundler left in target/release.
        assert!(joined.contains(&tarball_path(&info(), LINUX)), "{joined}");
        // And the bundler runs before it, or there is nothing to archive.
        assert!(
            joined.find("cargo tauri build") < joined.find("tar --sort=name"),
            "the bundler comes first: {joined}"
        );
    }

    #[test]
    fn the_tarball_ships_the_licence_the_binaries_and_every_icon_size() {
        let info = info();
        let joined = tarball_steps(&info, LINUX)
            .iter()
            .map(|s| s.display())
            .collect::<Vec<_>>()
            .join("\n");

        // A binary distribution that does not carry its licence is the problem `LICENSE` at the
        // root was added to fix; shipping it is the whole of the fix reaching this artefact.
        assert!(joined.contains("install -m644 LICENSE "), "{joined}");
        assert!(joined.contains("install -m644 README.md"), "{joined}");
        assert!(
            joined.contains(&format!("bin/{APP_BIN}"))
                && joined.contains(&format!("bin/{HOOK_BIN}")),
            "both binaries: {joined}"
        );
        for n in TARBALL_ICON_SIZES {
            assert!(
                joined.contains(&format!("hicolor/{n}x{n}/apps/{}.png", info.identifier)),
                "icon {n}: {joined}"
            );
        }
        // The generated entry, not a hand-written one — `package --check` is what keeps it true.
        assert!(
            joined.contains(&format!("{FLATPAK_DIR}/{}.desktop", info.identifier)),
            "{joined}"
        );
    }

    #[test]
    fn every_icon_the_tarball_installs_is_one_the_config_declares() {
        // The sizes are a literal here and a list in `tauri.conf.json`, and the drift is silent:
        // `install` would fail at run time on a size that was removed from `crates/cide-app/icons`,
        // twenty minutes into a release. The preflight's own icon check reads the config's list,
        // so this is what ties this literal to it.
        let info = read_app_info(&crate::workspace_root().expect("a workspace root"))
            .expect("the real config parses");
        for n in TARBALL_ICON_SIZES {
            let wanted = format!("icons/{n}x{n}.png");
            assert!(
                info.icons.contains(&wanted),
                "{wanted} is installed by the tarball but is not in {TAURI_CONF}'s bundle.icon: \
                 {:?}",
                info.icons
            );
        }
    }

    #[test]
    fn the_app_binary_is_what_the_crate_declares() {
        // `APP_BIN` and `APP_PACKAGE` are two different strings for one crate, and the tarball
        // installs `target/release/<APP_BIN>` — a name only `crates/cide-app/Cargo.toml` decides.
        let manifest = fs::read_to_string(
            crate::workspace_root()
                .expect("a workspace root")
                .join(APP_CRATE)
                .join("Cargo.toml"),
        )
        .expect("the app crate has a manifest");
        assert!(
            manifest.contains(&format!("name = \"{APP_PACKAGE}\"")),
            "package name"
        );
        assert!(
            manifest.contains(&format!("name = \"{APP_BIN}\"")),
            "[[bin]] name"
        );
    }

    #[test]
    fn the_source_tarball_is_the_only_target_no_host_refuses() {
        // Every other field of `Targets` names a bundler that exists on one platform. This one
        // is `git archive`, so `--src` on a Mac has to be a plan and not the cross-compilation
        // refusal — which is a different thing entirely from it being absent from that host's
        // *defaults*, and conflating the two would make the choice below unoverridable.
        let src_only = Targets {
            appimage: false,
            deb: false,
            flatpak: false,
            app: false,
            dmg: false,
            tarball: false,
            src: true,
            src_named: true,
        };
        assert!(src_only.impossible_on(LINUX).is_empty());
        assert!(src_only.impossible_on(MACOS).is_empty());
        assert_eq!(src_only.bundles(), None, "it is not a Tauri bundle");
    }

    #[test]
    fn only_one_host_produces_the_source_tarball_by_default() {
        // The one place `for_host` stops meaning "what this host can build". Two hosts producing
        // it means a release matrix uploading two files called `cide-0.1.0-src.tar.gz` whose
        // bytes differ — the tar layers are identical but the gzip layer belongs to whichever
        // compressor that runner shipped — and whichever upload lost would leave a published
        // checksum matching neither job's log.
        assert!(Targets::for_host(LINUX).src);
        assert!(!Targets::for_host(MACOS).src);
        // …and it is still not a refusal there, so `--src` on a Mac works.
        assert!(Targets::MACOS.impossible_on(MACOS).is_empty());

        let mac_default = plan(Path::new("/nonexistent"), &info(), Targets::MACOS, MACOS);
        assert!(
            mac_default.iter().all(|s| s.program != "git"),
            "a bare `package --run` on a Mac must not cut a second source tarball: {mac_default:?}"
        );
    }

    #[test]
    fn the_source_step_runs_before_anything_expensive() {
        // It costs a second and compiles nothing, while everything after it is a twenty-minute
        // build that can die inside a bundler. A release that lost its AppImage should still
        // have the source it was built from.
        let steps = plan(Path::new("/nonexistent"), &info(), Targets::LINUX, LINUX);
        let archive = steps
            .iter()
            .position(|s| s.program == "git")
            .expect("a step that archives the source");
        let first_cargo = steps
            .iter()
            .position(|s| s.program == "cargo")
            .expect("a step that compiles something");
        assert!(archive < first_cargo, "{steps:?}");
    }

    #[test]
    fn the_output_directory_is_created_first() {
        // `git archive -o` does not create it: measured, `fatal: could not open '…' for
        // writing: No such file or directory`, exit 128. Without the mkdir the whole plan fails
        // on its second command, after the preflight has said everything is fine.
        let steps = src_steps(&info());
        assert_eq!(steps[0].program, "mkdir");
        assert!(steps[0].args.contains(&SRC_BUNDLE_DIR.to_string()));
        assert_eq!(steps[1].program, "git");
        assert!(
            steps[1]
                .args
                .iter()
                .any(|a| a.starts_with(&format!("{SRC_BUNDLE_DIR}/"))),
            "the archive must land in the directory the mkdir made: {:?}",
            steps[1]
        );
    }

    #[test]
    fn the_tarball_is_named_and_prefixed_for_a_release_page() {
        let steps = src_steps(&info());
        let archive = &steps[1];
        assert_eq!(archive.args[0], "archive");
        assert!(
            archive.args.contains(&"--format=tar.gz".to_string()),
            "one argument, so the printed plan is pasteable: {archive:?}"
        );
        assert!(
            archive.args.last().is_some_and(|r| r == "HEAD"),
            "the archive is of a commit, never of the working tree: {archive:?}"
        );
        assert!(
            archive
                .args
                .iter()
                .any(|a| a.ends_with("cide-0.1.0-src.tar.gz")),
            "the name carries the version, because an asset is downloaded away from its \
             release page: {archive:?}"
        );

        // The prefix is what stops a tarbomb: without it, unpacking scatters ~900 files across
        // whatever directory the user was in. `git archive` prepends the string verbatim, so
        // the trailing slash is load-bearing rather than cosmetic.
        let prefix = archive
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--prefix="))
            .expect("a --prefix, or the tarball unpacks into the current directory");
        assert_eq!(prefix, "cide-0.1.0/");
        assert!(prefix.ends_with('/'));
    }

    #[test]
    fn asking_for_src_alone_builds_no_hook_and_runs_no_bundler() {
        let targets = Targets {
            appimage: false,
            deb: false,
            flatpak: false,
            app: false,
            dmg: false,
            tarball: false,
            src: true,
            src_named: true,
        };
        let steps = plan(Path::new("/nonexistent"), &info(), targets, LINUX);
        assert_eq!(
            steps.len(),
            2,
            "mkdir and git archive, nothing else: {steps:?}"
        );
        assert!(
            steps.iter().all(|s| s.program != "cargo"),
            "nothing is compiled to produce a source tarball: {steps:?}"
        );
        assert!(steps.iter().all(|s| s.program != "curl"), "{steps:?}");
        assert!(
            steps.iter().all(|s| s.program != "flatpak-builder"),
            "{steps:?}"
        );
    }

    #[test]
    fn a_dirty_tree_is_a_failure_and_names_the_commit_and_the_files() {
        // The failure this whole preflight exists for. `git archive` packages HEAD, so a tree
        // with uncommitted work produces a valid tarball of something nobody is looking at —
        // a false claim about a commit, discovered later as a checksum that matches nothing.
        let mut status = clean();
        status.dirty = vec!["crates/cide-app/src/lib.rs".into(), "ui/src/App.tsx".into()];
        status.dirty_total = 2;
        let verdicts = src_verdicts(&status, &info(), true);
        let failure = verdicts
            .iter()
            .find(|v| matches!(v, Verdict::Fail(_)))
            .unwrap_or_else(|| panic!("a dirty tree must fail: {verdicts:?}"));
        let detail = failure.detail();
        assert!(
            detail.contains("144260a"),
            "it has to name the commit that would be archived: {detail}"
        );
        assert!(
            detail.contains("crates/cide-app/src/lib.rs") && detail.contains("ui/src/App.tsx"),
            "and the files, or the reader goes to `git status` to learn what this already \
             knows: {detail}"
        );
        assert!(
            detail.contains("stash") || detail.contains("Commit"),
            "and what to do about it: {detail}"
        );
    }

    /// A dirty tree the caller never asked about warns, and the archive step is dropped.
    ///
    /// The other half of the verdict above, and the two must agree or the feature is worse than
    /// either. `src` is in `Targets::LINUX`, so a bare `cargo xtask package --run` — which is what
    /// `./build.sh --all` invokes — inherits it. Failing there means a developer testing an
    /// AppImage of their work in progress builds nothing at all, and reads advice to commit or
    /// stash the very changes they are testing.
    ///
    /// A bare `./build.sh` passes `--appimage` and so never reaches this case at all, which is the
    /// point of naming the target: the common reason to run that script is to try uncommitted
    /// work, and the tarball is not what the caller came for. This test still guards the path,
    /// because `--all` is one flag away and the release run is exactly the default set.
    ///
    /// Warning and leaving the step in the plan would be the worse mistake still: it would print
    /// a caution and then produce the exact artefact the caution is about. So this asserts BOTH —
    /// the verdict is a warning, and the plan has no archive step.
    #[test]
    fn an_unrequested_tarball_warns_on_a_dirty_tree_and_is_skipped() {
        let mut status = clean();
        status.dirty = vec!["crates/cide-app/src/lib.rs".into()];
        status.dirty_total = 1;

        let verdicts = src_verdicts(&status, &info(), false);
        let warn = verdicts
            .iter()
            .find_map(|v| match v {
                Verdict::Warn(text) if text.contains("uncommitted") => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("an unrequested tarball warns rather than fails: {verdicts:?}")
            });
        assert!(
            warn.contains("skipped") && warn.contains("--src"),
            "the warning says what was skipped and how to ask for it deliberately: {warn}"
        );
        assert!(
            !verdicts.iter().any(|v| matches!(v, Verdict::Fail(_))),
            "nothing fails, so every other target still builds: {verdicts:?}"
        );

        // ...and the same tree, named explicitly, is still a refusal. A release artefact that
        // does not match HEAD is the thing this preflight exists to stop.
        assert!(
            src_verdicts(&status, &info(), true)
                .iter()
                .any(|v| matches!(v, Verdict::Fail(_))),
            "asking for it by name still refuses"
        );
    }

    /// The plan half of the dirty-tree decision, which was the untested one.
    ///
    /// Mutation-checked: deleting the skip from `plan` left all 89 other tests green, because
    /// `plan` reads the live repository and nothing could fixture it. The rule moved out so this
    /// can drive it.
    #[test]
    fn a_dirty_tree_only_cuts_a_tarball_that_was_asked_for() {
        let inherited = Targets {
            src: true,
            src_named: false,
            ..Targets::LINUX
        };
        let asked = Targets {
            src: true,
            src_named: true,
            ..Targets::LINUX
        };

        assert!(
            archives_source(inherited, 0),
            "a clean tree cuts one either way"
        );
        assert!(archives_source(asked, 0));
        assert!(
            !archives_source(inherited, 1),
            "an inherited tarball is SKIPPED on a dirty tree — the warning that replaced the \
             failure is only true if the step really goes"
        );
        assert!(
            archives_source(asked, 1),
            "asked for by name it is still planned; `src_verdicts` fails the run instead, which \
             is where the user is told why"
        );
        assert!(
            !archives_source(
                Targets {
                    src: false,
                    ..asked
                },
                0
            ),
            "and not wanting it at all still means not cutting one"
        );
    }

    #[test]
    fn a_long_dirty_list_is_capped_and_says_how_many_it_hid() {
        let mut status = clean();
        status.dirty = (0..NAMED_FILES).map(|i| format!("f{i}.rs")).collect();
        status.dirty_total = 40;
        let detail = src_verdicts(&status, &info(), true)
            .into_iter()
            .find_map(|v| match v {
                Verdict::Fail(d) => Some(d),
                _ => None,
            })
            .expect("a failure");
        assert!(detail.contains("40 tracked file(s)"), "{detail}");
        assert!(
            detail.contains("and 35 more"),
            "forty paths would bury the sentence that matters: {detail}"
        );
    }

    #[test]
    fn an_untracked_file_is_a_warning_not_a_failure() {
        // They cannot reach the archive either, but they are usually scratch. The narrow
        // dangerous case — a new source file the committed code already imports — is worth a
        // sentence, not a refusal.
        let mut status = clean();
        status.untracked = vec!["ui/src/chrome/tabDrag.ts".into()];
        status.untracked_total = 1;
        let verdicts = src_verdicts(&status, &info(), true);
        assert!(
            verdicts.iter().all(|v| !matches!(v, Verdict::Fail(_))),
            "an untracked scratch file must not refuse a release: {verdicts:?}"
        );
        assert!(
            verdicts.iter().any(|v| matches!(v, Verdict::Warn(d)
                if d.contains("ui/src/chrome/tabDrag.ts") && d.contains("will not build"))),
            "{verdicts:?}"
        );
    }

    #[test]
    fn an_unpacked_tarball_cannot_produce_another_one() {
        // Measured in an extracted tree: `git rev-parse --is-inside-work-tree` exits 128. The
        // tarball ships no `.git`, so this is the state anyone who downloaded one is in, and
        // "not a git checkout" is a far better answer than whatever `git archive` says.
        let status = SrcStatus {
            git: true,
            ..SrcStatus::default()
        };
        let verdicts = src_verdicts(&status, &info(), true);
        assert!(
            verdicts
                .iter()
                .any(|v| matches!(v, Verdict::Fail(d) if d.contains("not a git checkout"))),
            "{verdicts:?}"
        );
        assert_eq!(verdicts.len(), 1, "nothing else is knowable: {verdicts:?}");
    }

    #[test]
    fn a_nested_checkout_is_refused() {
        // A workspace vendored inside somebody else's repository. `git archive HEAD` run from
        // here packages *that* project, and the result looks entirely plausible: right file
        // name, right version, wrong contents.
        let mut status = clean();
        status.toplevel = Some("/home/dev/monorepo".into());
        status.toplevel_is_root = false;
        let verdicts = src_verdicts(&status, &info(), true);
        assert!(
            verdicts
                .iter()
                .any(|v| matches!(v, Verdict::Fail(d) if d.contains("/home/dev/monorepo"))),
            "the refusal must name the repository it found: {verdicts:?}"
        );
    }

    #[test]
    fn a_missing_git_is_a_failure_that_says_why_a_tar_would_not_do() {
        let verdicts = src_verdicts(&SrcStatus::default(), &info(), true);
        let detail = verdicts.first().expect("a verdict").detail();
        assert!(detail.contains("git is not on PATH"), "{detail}");
        assert!(
            detail.contains("target/") || detail.contains("node_modules"),
            "a reader will reach for `tar` next; the answer is why that ships the wrong \
             files: {detail}"
        );
    }

    #[test]
    fn a_configured_compressor_is_a_warning() {
        // Measured: `git -c tar.tar.gz.command='gzip -c' archive` is deterministic across runs
        // but produces different bytes and a different size from git's built-in (3,041,136 vs
        // 3,044,533). It is the only thing on a machine that can change the tarball for a
        // fixed commit, so it is the only reproducibility caveat worth printing.
        let mut status = clean();
        status.compressor = Some("gzip -c".into());
        let verdicts = src_verdicts(&status, &info(), true);
        assert!(
            verdicts.iter().any(|v| matches!(v, Verdict::Warn(d)
                if d.contains("tar.tar.gz.command") && d.contains("gzip -c"))),
            "{verdicts:?}"
        );
        assert!(
            verdicts.iter().all(|v| !matches!(v, Verdict::Fail(_))),
            "a different-but-valid gzip is not a reason to refuse: {verdicts:?}"
        );
    }

    #[test]
    fn an_untagged_head_is_a_warning_and_never_a_failure() {
        // `actions/checkout` fetches no tags without `fetch-depth: 0`, so a failure here would
        // make every CI packaging run red for a reason that is about the checkout.
        let mut status = clean();
        status.tags = Vec::new();
        let verdicts = src_verdicts(&status, &info(), true);
        assert!(
            verdicts
                .iter()
                .any(|v| matches!(v, Verdict::Warn(d) if d.contains("0.1.0"))),
            "{verdicts:?}"
        );
        assert!(
            verdicts.iter().all(|v| !matches!(v, Verdict::Fail(_))),
            "{verdicts:?}"
        );

        // A tag that is not this version does not count, and both spellings of one that is do.
        // `v` is the dominant convention and a bare version is common enough that accepting
        // only one would warn about half the correctly tagged trees there are.
        let with = |tag: &str| {
            let mut s = clean();
            s.tags = vec![tag.into()];
            src_verdicts(&s, &info(), true)
        };
        assert_eq!(with("v0.1.0").len(), 1, "{:?}", with("v0.1.0"));
        assert_eq!(with("0.1.0").len(), 1, "{:?}", with("0.1.0"));
        assert!(
            with("v0.2.0").iter().any(|v| matches!(v, Verdict::Warn(_))),
            "a tag for another release is not this release's tag: {:?}",
            with("v0.2.0")
        );
    }

    #[test]
    fn a_clean_checkout_says_exactly_what_it_will_produce() {
        let verdicts = src_verdicts(&clean(), &info(), true);
        assert_eq!(
            verdicts.len(),
            1,
            "a clean tagged checkout has nothing to warn about: {verdicts:?}"
        );
        let Verdict::Ok(detail) = &verdicts[0] else {
            panic!("{verdicts:?}");
        };
        assert!(detail.contains("144260a"), "{detail}");
        assert!(
            detail.contains("target/release/bundle/src/cide-0.1.0-src.tar.gz"),
            "{detail}"
        );
        assert!(
            detail.contains("cide-0.1.0/"),
            "and what it unpacks as: {detail}"
        );
    }

    /// # Why this one returns early instead of asserting unconditionally
    ///
    /// It runs against the actual repository rather than a fixture, and the actual repository is
    /// not always a git checkout: **the source tarball this feature produces unpacks into a tree
    /// with no `.git`**, and `CLAUDE.md` lists `cargo test --workspace` as a gate a consumer is
    /// expected to run. Asserted unconditionally, the artifact could not pass its own documented
    /// suite — a distro packager, a Homebrew formula or a Nix `checkPhase` would see one red test
    /// and reasonably read it as a broken release rather than as a missing directory.
    ///
    /// The early return is not a hole. `an_unpacked_tarball_cannot_produce_another_one` covers
    /// exactly the state this skips, with a fixture, so the no-git path is asserted either way;
    /// what only a real checkout can check is that `read_src_status` still gets a true answer out
    /// of a live git, and that is what remains below.
    #[test]
    fn the_real_checkout_is_one_a_source_tarball_can_be_cut_from() {
        let root = crate::workspace_root().expect("a workspace root");
        let status = read_src_status(&root);
        if !status.git || !root.join(".git").exists() {
            eprintln!("skipped: not a git checkout — this is the unpacked-tarball case");
            return;
        }
        assert!(
            status.toplevel_is_root,
            "the workspace root must be the git root: {status:?}"
        );
        assert!(status.head.is_some(), "{status:?}");
    }

    #[test]
    fn an_artefact_listing_finds_the_tarball() {
        // `Path::extension` answers `"gz"` for `cide-0.1.0-src.tar.gz`, so the extension list
        // the other four artefacts use silently drops it — built, never printed, never
        // uploaded. That is the whole reason this is a named function.
        assert!(is_artefact("cide-0.1.0-src.tar.gz"));
        assert!(is_artefact("cide_0.1.0_amd64.AppImage"));
        assert!(is_artefact("cide_0.1.0_amd64.deb"));
        assert!(is_artefact("cide.app"));
        assert!(is_artefact("cide_0.1.0_aarch64.dmg"));
        // …and not everything else that ends up in that tree.
        assert!(!is_artefact("cide-0.1.0-src.tar"));
        assert!(!is_artefact("build.log.gz"));
        assert!(!is_artefact("cide"));
    }

    #[test]
    fn the_printed_checksum_is_the_one_sha256sum_would_print() {
        // The number goes onto a release page for people to verify with `sha256sum -c`. A
        // different-but-respectable digest would be worse than none, so this is pinned to the
        // NIST vector for "abc" rather than to whatever the code currently computes.
        let dir = std::env::temp_dir().join(format!("cide-xtask-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        // A directory — which is what a macOS `.app` is — must not produce a checksum line.
        assert_eq!(sha256_file(&dir), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_capped_list_reads_as_a_sentence() {
        assert_eq!(name_some(&["a".into()], 1), "a");
        assert_eq!(name_some(&["a".into(), "b".into()], 2), "a, b");
        assert_eq!(name_some(&["a".into(), "b".into()], 7), "a, b and 5 more");
    }
}
