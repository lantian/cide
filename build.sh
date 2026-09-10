#!/usr/bin/env bash
#
# Build a distributable cide bundle for THIS host, and put it where you can run it.
#
# `cargo xtask package` is the whole packaging brain: it reads `rustc -vV` to learn the host,
# preflights the configuration, and refuses a target the machine cannot produce, because nothing
# here cross-compiles (docs/adr/0007). This script is the convenience layer over it — pick the one
# artefact a person actually wants on this platform, build it, and install it:
#
#   Linux  →  target/release/bundle/appimage/cide_<version>_<arch>.AppImage, copied into
#             $CIDE_INSTALL_DIR (default ~/bin). An AppImage is a single executable file, so a
#             directory on PATH is the whole of "installing" it.
#   macOS  →  target/release/bundle/dmg/cide_<version>_<arch>.dmg, and copied nowhere by default:
#             a .dmg is an installer you open, not a binary for PATH. Set CIDE_INSTALL_DIR to
#             have it copied somewhere as well.
#
# **macOS has never been built or run.** `docs/platforms.md` is the record and this script
# does not pretend otherwise. The Darwin branch is the documented plan and not an observation:
# `dmg` is in `Targets::MACOS`, `cargo tauri build --bundles dmg` runs the .app bundler first, and
# `tauri.macos.conf.json` overrides `bundle.targets` so a Mac does not try to build an AppImage.
# It should work on the first Mac that tries it; nobody has.
#
# There is deliberately no copy into /Applications. That would be an unverified claim about a
# platform nobody has run — installing a .app has rules (quarantine, signature, Gatekeeper) this
# script is in no position to assert, and the preflight's own signing verdict is the honest
# statement of what an ad-hoc-signed local bundle does there.
#
#   ./build.sh                # the artefact this host is for: AppImage on Linux, .dmg on macOS
#   ./build.sh --all          # everything this host is responsible for — on Linux that adds the
#                             #   .deb, the Flatpak and the source tarball
#   ./build.sh --plan         # preflight and print the plan; build nothing
#   ./build.sh --no-install   # build, but do not copy the result anywhere
#
# Not `set -e`: every step below is checked by hand, so each failure can say what it means for
# the artefact rather than exiting on a line number.
set -uo pipefail

cd "$(dirname "$0")"

usage() {
  cat >&2 <<'EOF'
  ./build.sh                # the artefact this host is for: AppImage on Linux, .dmg on macOS
  ./build.sh --all          # everything this host is responsible for
  ./build.sh --plan         # preflight and print the plan; build nothing
  ./build.sh --no-install   # build, but do not copy the result anywhere
EOF
}

all=0
plan_only=0
install=1

for arg in "$@"; do
  case "$arg" in
    --all)        all=1 ;;
    --plan)       plan_only=1 ;;
    --no-install) install=0 ;;
    -h|--help)    usage; exit 0 ;;
    *) echo "[build] unknown option: $arg" >&2; usage; exit 2 ;;
  esac
done

# --- which artefact this host is for ---------------------------------------------------
#
# `uname -s` and not `cfg!`/`$OSTYPE`: the packaging task decides the same question from
# `rustc -vV` and refuses a mismatch, so this only has to agree with it well enough to name the
# right flag. Getting it wrong is a preflight failure with a reason, not a broken bundle.
host=$(uname -s)
case "$host" in
  Linux)
    target_flag=--appimage
    bundle_dir=target/release/bundle/appimage
    pattern='*.AppImage'
    install_dir=${CIDE_INSTALL_DIR:-$HOME/bin}
    ;;
  Darwin)
    target_flag=--dmg
    bundle_dir=target/release/bundle/dmg
    pattern='*.dmg'
    # No default, for the reason in the header. Honoured when the caller names one, because then
    # where a .dmg lands is their decision rather than this script's guess.
    install_dir=${CIDE_INSTALL_DIR:-}
    ;;
  *)
    echo "[build] $host is not a platform cide targets." >&2
    echo "[build] Linux and macOS only, and neither cross-compiles to the other." >&2
    echo "[build] See docs/platforms.md." >&2
    exit 2
    ;;
esac

# Naming the target rather than taking the host's default set is what makes a bare `./build.sh`
# mean one artefact. It has a second effect worth knowing: `src` is in `Targets::LINUX`, so a
# default run inherits the source tarball, and a dirty working tree then costs a warning and
# drops that step. Asking for the AppImage alone never raises the question — which is the common
# case, because the reason to run this is usually to try uncommitted work.
#
# `--locked` leads, and it has to: `xtask` is a `.cargo/config.toml` alias, so it is cargo's flag
# and not the task's. A re-resolved lockfile is the drift class the flag exists to catch.
package=(cargo --locked xtask package)
[ "$all" = 1 ] || package+=("$target_flag")

if [ "$plan_only" = 1 ]; then
  echo "[build] ${package[*]}"
  exec "${package[@]}"
fi

# --- build ------------------------------------------------------------------------------
#
# A marker file, stamped before the build, to compare the artefact against afterwards.
# `target/release/bundle/` keeps every bundle ever made there, including ones for versions this
# checkout no longer builds, so "the newest .AppImage in that directory" is not on its own
# evidence that this run produced anything.
#
# This script used to be an unconditional `cp` after the build, which is precisely that bug: a
# packaging run that failed still installed whatever was left over from the last one, reported
# success, and the stale AppImage then behaved like a new build that had mysteriously ignored
# the change under test.
tmp=
marker=$(mktemp "${TMPDIR:-/tmp}/cide-build.XXXXXX") || exit 1
# A function rather than a one-line trap body, so both paths stay quoted: an install directory
# with a space in it would otherwise reach `rm` as two arguments.
cleanup() {
  rm -f "$marker"
  if [ -n "$tmp" ]; then rm -f "$tmp"; fi
}
trap cleanup EXIT

echo "[build] ${package[*]} --run"
if ! "${package[@]}" --run; then
  echo "[build] REFUSING to install: the packaging run failed, so nothing new was produced." >&2
  echo "[build] Its own output is above, and nothing was copied anywhere." >&2
  exit 1
fi

# --- find what it produced --------------------------------------------------------------
#
# Globbed rather than spelled out, because the name carries the version and the architecture:
# tauri writes `cide_<version>_<arch>.AppImage`, the version comes from
# `crates/cide-app/tauri.conf.json`, and this script hardcoded `cide_0.1.0_amd64.AppImage` until
# it did not — which would have installed 0.2.0 under the old name on the day the version moved,
# and found nothing at all on an arm64 machine.
shopt -s nullglob
matches=("$bundle_dir"/$pattern)
shopt -u nullglob

if [ "${#matches[@]}" = 0 ]; then
  echo "[build] the packaging run succeeded but $bundle_dir holds no $pattern." >&2
  exit 1
fi

# Newest wins. `-nt` and not `ls -t` or `stat`: parsing ls breaks on a space in a path, and
# `stat` takes `-c %Y` on GNU and `-f %m` on BSD, which is exactly the split this script exists
# to stop caring about.
artefact=${matches[0]}
for f in "${matches[@]}"; do
  [ "$f" -nt "$artefact" ] && artefact=$f
done

if [ ! "$artefact" -nt "$marker" ]; then
  echo "[build] $artefact predates this run, so the bundler produced nothing new." >&2
  echo "[build] Not installing a stale artefact; re-run the plan to see why:" >&2
  echo "[build]   cargo --locked xtask package $target_flag" >&2
  exit 1
fi

echo "[build] built $artefact"

# --- install ----------------------------------------------------------------------------

if [ "$install" = 0 ]; then
  echo "[build] --no-install: left in $bundle_dir"
  exit 0
fi

if [ -z "$install_dir" ]; then
  # macOS with no CIDE_INSTALL_DIR: say what to do with the thing rather than nothing at all.
  echo "[build] open it with:  open \"$artefact\""
  echo "[build] (set CIDE_INSTALL_DIR to have the .dmg copied there too)"
  exit 0
fi

# Not created here. A typo in CIDE_INSTALL_DIR would otherwise silently produce a directory
# nobody wants and an "installed" line pointing into it.
if [ ! -d "$install_dir" ]; then
  echo "[build] $install_dir does not exist, so $artefact was not installed." >&2
  echo "[build] Create it, or point CIDE_INSTALL_DIR at a directory that does." >&2
  exit 1
fi

dest=$install_dir/$(basename "$artefact")

# Written beside the destination and renamed onto it, rather than copied over it. A running
# AppImage holds its own file open, and writing into a busy executable fails with ETXTBSY
# ("Text file busy") on Linux — rebuilding while the last build is still on screen is the normal
# case here, not an edge one. `mv` inside one directory is a rename: it swaps the directory
# entry, the running process keeps the inode it already had, and no reader ever sees a
# half-written file.
#
# Measured, not assumed: `cp` over a running executable fails with `cp: cannot create regular
# file '...': Text file busy` and exit 1, which is what the previous version of this script did.
tmp=$dest.new.$$
if ! cp "$artefact" "$tmp"; then
  echo "[build] could not write into $install_dir; $dest is unchanged." >&2
  exit 1
fi

# The bundler already sets it and `cp` preserves it; this is for a destination that dropped it
# (a mount with `noexec` off but a restrictive umask, a filesystem without the bit).
case "$artefact" in
  *.AppImage) chmod +x "$tmp" ;;
esac

if ! mv -f "$tmp" "$dest"; then
  echo "[build] could not replace $dest." >&2
  exit 1
fi
tmp=

echo "[build] installed $dest"
