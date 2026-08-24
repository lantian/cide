#!/usr/bin/env bash
# Bump the product version in every file that carries it.
#
#   scripts/bump-version.sh          # 0.2.0 -> 0.3.0 (minor +1, patch reset)
#   scripts/bump-version.sh 1.0.0    # set an explicit version
#
# Four files, three formats, and none of them derives from the others — the same drift
# the release workflow's prepare step exists to end. Before that step existed, Cargo.toml
# said 0.2.0 and tauri.conf.json said 0.1.0, and since every artefact name comes from
# tauri.conf.json, a release tagged v0.2.0 would have shipped cide_0.1.0_amd64.AppImage.
# The edit logic below is the workflow's, kept byte-compatible on purpose: if one of them
# grows a new rule the other must grow it too (.github/workflows/release.yml, "Write the
# version into every file that carries it").
#
# What this script deliberately does NOT do is the workflow's git-diff guard ("exactly
# one line changed per file"): locally the tree is allowed to be dirty, so a diff-based
# check would fail on unrelated in-progress work. The python asserts (exactly one match,
# and the file still parses to the new version) are the safety that transfers.

set -euo pipefail
cd "$(dirname "$0")/.."

current=$(python3 -c '
import re
src = open("Cargo.toml", encoding="utf-8").read()
m = re.search(r"\[workspace\.package\][^\[]*?\nversion = \"([^\"]*)\"", src, re.S)
assert m, "no version key under [workspace.package] in Cargo.toml"
print(m.group(1))
')

if [ $# -gt 1 ]; then
  echo "usage: $0 [version]" >&2
  exit 2
elif [ $# -eq 1 ]; then
  version="$1"
  # Same rule as the release workflow: a version is a file name people will read, and a
  # stray "v" prefix would ship cide_v1.0.0_amd64.AppImage.
  if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
    echo "error: '$version' is not a semver version. Write 0.2.0, not v0.2.0." >&2
    exit 2
  fi
else
  # No argument: minor bump, patch reset, any -rc/+build suffix dropped. That is the
  # release cadence this project actually has; a patch release is rare enough to be typed.
  version=$(printf '%s' "$current" | python3 -c '
import re, sys
m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:[-+].*)?", sys.stdin.read().strip())
assert m, "current version is not semver"
print(f"{m.group(1)}.{int(m.group(2)) + 1}.0")
')
fi

if [ "$version" = "$current" ]; then
  echo "already at $current, nothing to do"
  exit 0
fi

echo "bumping $current -> $version"

python3 - "$version" <<'PY'
import json, re, sys

version = sys.argv[1]

# Anchored to the [workspace.package] table. A bare `s/^version = .*/` would also
# rewrite the first version key of whatever table happens to come first in the file.
path = "Cargo.toml"
src = open(path, encoding="utf-8").read()
out, n = re.subn(
    r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]*(")',
    lambda m: m.group(1) + version + m.group(2),
    src,
    count=1,
    flags=re.S,
)
assert n == 1, f"no version key under [workspace.package] in {path}"
open(path, "w", encoding="utf-8").write(out)

# A line rewrite and NOT a json round-trip, which is what this was first and which was
# wrong: `json.dump(indent=2)` reformats the whole document, and tauri.conf.json keeps
# short arrays inline — `"targets": ["appimage", "deb"]` came back as four lines. The
# version was correct and the diff was twenty lines of reflow inside a release tag.
#
# `^  "version":` is exact enough to be safe: both files are indented two spaces, so a
# key at that indent is a top-level key, and a nested "version" would be at four or
# more. The parse below is what makes that a checked claim rather than a hopeful one.
for path in ("crates/cide-app/tauri.conf.json", "ui/package.json"):
    src = open(path, encoding="utf-8").read()
    out, n = re.subn(
        r'^(  "version": ")[^"]*(")',
        lambda m: m.group(1) + version + m.group(2),
        src,
        count=1,
        flags=re.M,
    )
    assert n == 1, f"no top-level version line in {path}"
    open(path, "w", encoding="utf-8").write(out)
    assert json.load(open(path, encoding="utf-8"))["version"] == version, (
        f"{path} still parses, but its top-level version is not {version} — the line "
        f"that matched was not the one that means the product's version"
    )
PY

# Cargo.lock records every workspace member's version, and CI passes --locked
# everywhere. Without this the next build fails with "the lock file needs to be
# updated but --locked was passed". --offline: nothing new is being resolved.
cargo update --workspace --offline

# packaging/flatpak/* is generated from tauri.conf.json's AppInfo — the AppStream
# metainfo publishes the version as a <release>. `package --check` is the CI gate that
# fails when they drift, so regenerate, then prove the tree passes its own gates.
cargo --locked xtask package --write
cargo --locked xtask package --check
cargo --locked metadata --format-version 1 --offline > /dev/null

echo "done: $current -> $version"
git diff --stat -- Cargo.toml Cargo.lock crates/cide-app/tauri.conf.json ui/package.json packaging/flatpak
