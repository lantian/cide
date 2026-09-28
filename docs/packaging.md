# Packaging and releases

`CONTRIBUTING.md` has the commands; this is the detail behind them.

## `cargo xtask package`

```sh
cargo xtask package [--appimage|--deb|--flatpak|--tarball|--app|--dmg|--src] [--check|--write|--run]
```

Without `--run` it only prints a plan. Naming no target means everything *this host* is
responsible for; naming a bundle the host cannot build (`--dmg` on Linux) is a preflight failure,
because nothing here cross-compiles.

**Every packaged target needs the sibling forks checked out at the revisions `packaging/*.lock`
pin** — preflight fails with the clone command when one is absent; `./scripts/clone-forks.sh`
checks them out, and `docs/forks.md` is the whole story.

- **`--src`** is the exception and the cheap one: `git archive` of HEAD into
  `target/release/bundle/src/cide-<version>-src.tar.gz`, byte-reproducible, buildable on any host
  but in the **Linux** default set only, so a release matrix uploads one source tarball rather
  than two that differ. It refuses a dirty tree — a tarball cut from one is a false claim about a
  commit.
- **`--tarball`** is the *binary* one and makes the opposite promise:
  `target/release/bundle/tarball/cide-<version>-linux-<arch>.tar.gz`, the three binaries (`cide`,
  `cide-hook`, and since M25 `cide-rust-analyzer` — cide's own rust-analyzer build from the sibling
  fork pinned by `packaging/rust-analyzer.lock`) plus the desktop entry, icons, README and LICENSE
  under a `cide-<version>/` prefix. It carries no WebKitGTK, so it needs the host's 4.1 — but the
  bundled rust-analyzer means it is no longer "a tenth the size of the AppImage"; both archives
  carry the same ~50 MiB fork. Compiled output, so nothing about its bytes is reproducible. Linux
  only.

Icon drift is checked separately: `scripts/gen-icons.sh --check`.

## `./build.sh`

The wrapper for daily use: one artefact for the host (AppImage on Linux, `.dmg` on macOS), then
installs it into `$CIDE_INSTALL_DIR` (default `~/bin`, Linux only — a `.dmg` is opened, not put on
PATH). `--all` for the host's whole default set, `--plan` to build nothing, `--no-install` to stop
after building.

## Releases

**Releases are `.github/workflows/release.yml`**, dispatched by hand with a version: it cuts
`release/v<version>` from master, writes that version into the four files that carry it
(Cargo.toml, tauri.conf.json, ui/package.json, and the regenerated flatpak metainfo — they drifted
before it existed), tags, builds every artefact through `cargo xtask package`, and publishes one
GitHub release with a SHA256SUMS.

**Nothing reaches the remote until every build has passed.** The release commit and tag travel
between the jobs as a git bundle (`.github/actions/release-commit`), and `publish` pushes the
branch and the tag (`--atomic`) just before creating the release. A failed build therefore leaves
no branch or tag behind — dispatch the same version again. (Until 2026-09-28 `prepare` pushed
both first, and every failed build meant deleting them by hand on the remote.)

**Never bump the version by hand for a release, and never bump master to the version you are
about to release** — that file then differs by no lines and the workflow's `1 1` numstat guard
refuses the run. A final `bump` job puts master on `<next patch>-dev` after publishing, because the
release branch is never merged back and the trunk otherwise keeps a version two releases old;
`scripts/bump-version.sh` is the same edit by hand, and release.yml now calls it.

## Self-update

Installed copies check `https://github.com/lantian/cide/releases/latest/download/latest.json` a
few seconds after start (`crates/cide-app/src/updater.rs`). GitHub's `latest` skips drafts and
prereleases, so only stable releases are ever offered. The user can **Update** (download, verify,
replace, then asked whether to restart), **Skip this version** (`settings.update.skippedVersion`),
or dismiss the notice until the next start. *Check for updates* in the palette and the About card
asks on demand and ignores a skip. Settings › Projects & windows turns the start-up check off.

Only the AppImage and a `.app` in a writable, non-translocated location replace themselves. The
tarball, a `.deb`, the Flatpak and a copy running from the `.dmg` get *Open release page*.
Development builds (`-dev` versions, debug builds) never check.

### One-time setup (the maintainer)

1. `pnpm --dir ui exec tauri signer generate -w ~/.tauri/cide.key` — keep the private key and its
   password safe; losing it means installed copies can never be updated again.
2. Repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the key file's contents) and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Paste the public key (`~/.tauri/cide.key.pub`'s contents) into `plugins.updater.pubkey` in
   `crates/cide-app/tauri.conf.json` and commit it.

Until step 3 the running app never checks, and **`release.yml` refuses to build**: it sets
`CIDE_RELEASE=1`, under which `xtask package`'s preflight fails an unsigned release instead of
warning, because an unsigned release publishes no manifest and installed copies go quiet with no
error anywhere.

### What a release publishes for it

- `xtask package` passes `{"bundle":{"createUpdaterArtifacts":true}}` as an extra `--config` when
  `TAURI_SIGNING_PRIVATE_KEY` is set (never in `tauri.conf.json`, where it would break every
  unsigned local build). The bundler then writes `<AppImage>.sig`, and on macOS
  `bundle/macos/cide.app.tar.gz` and its `.sig`; the macos job renames those to
  `cide_<version>_<arch>.app.tar.gz[.sig]`.
- `scripts/updater-manifest.py` writes `latest.json` in the publish job from those `.sig` files:
  `version`, `pub_date`, and `platforms.{linux-x86_64,darwin-aarch64}.{signature,url}`.

### Testing an update end to end

Needs two builds stamped with release versions (not `-dev`) and signed with one throwaway key:

```sh
pnpm --dir ui exec tauri signer generate -w /tmp/test.key    # paste its .pub into tauri.conf.json, uncommitted
export TAURI_SIGNING_PRIVATE_KEY="$(cat /tmp/test.key)" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=…
# version 0.10.1 in tauri.conf.json → cargo xtask package --appimage --run → keep the AppImage
# version 0.10.3 → build again → copy its AppImage and .sig into ./srv
python3 scripts/updater-manifest.py --dir srv --version 0.10.3 --repo lantian/cide
# edit srv/latest.json's url to http://127.0.0.1:8000/<AppImage>, serve srv with python3 -m http.server
CIDE_PROFILE=test CIDE_UPDATE_ENDPOINT=http://127.0.0.1:8000/latest.json ./cide_0.10.1_amd64.AppImage
```

The plugin refuses plain `http` endpoints unless `plugins.updater.dangerousInsecureTransportProtocol`
is `true`; set it in the throwaway build's config only.

