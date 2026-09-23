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

**Never bump the version by hand for a release, and never bump master to the version you are
about to release** — that file then differs by no lines and the workflow's `1 1` numstat guard
refuses the run. A final `bump` job puts master on `<next patch>-dev` after publishing, because the
release branch is never merged back and the trunk otherwise keeps a version two releases old;
`scripts/bump-version.sh` is the same edit by hand, and release.yml now calls it.
