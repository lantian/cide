#!/usr/bin/env bash
# Check out the three sibling forks a packaged build needs, each at the revision its lock
# pins. Idempotent: an existing checkout is fetched and moved to the pin rather than refused.
#
# Nothing in a *dev* build needs any of this — `./run.sh` uses the PATH rust-analyzer and the
# PATH gopls. It is `cargo xtask package` that needs them, and its preflight prints the clone
# command for whichever one is missing; this script is that command for all three at once, so
# nobody has to run the preflight three times to discover the third.
#
# The pins live in packaging/*.lock and nowhere else — this script has no URLs of its own,
# because a second copy of a pin is a pin that can disagree with the one the release uses.
#
# The layout is not a preference. `../forks/rust-analyzer/Cargo.toml` carries
# `[patch.crates-io] salsa = { path = "../salsa" }`, and cargo resolves that against the
# *fork*, so rust-analyzer and salsa must be siblings of each other. `FORKS_DIR` moves the
# pair together or not at all.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
forks="${FORKS_DIR:-$root/../forks}"

mkdir -p "$forks"
forks="$(cd "$forks" && pwd)"

# name:lock:url-var:rev-var — the directory name is the one the locks, the preflight and
# release.yml all already assume.
entries=(
  "rust-analyzer:packaging/rust-analyzer.lock:CIDE_RA_URL:CIDE_RA_REV"
  "salsa:packaging/salsa.lock:CIDE_SALSA_URL:CIDE_SALSA_REV"
  "tools:packaging/gopls.lock:CIDE_GOPLS_URL:CIDE_GOPLS_REV"
)

for entry in "${entries[@]}"; do
  IFS=: read -r dir lock url_var rev_var <<<"$entry"
  # shellcheck disable=SC1090
  . "$root/$lock"
  url="${!url_var}"
  rev="${!rev_var}"
  dest="$forks/$dir"

  if [ ! -d "$dest/.git" ]; then
    echo "==> cloning $url into $dest"
    # --filter=blob:none, not --depth: rust-analyzer's history is large and a shallow clone
    # cannot be rebased onto a newer upstream tag later, which is the one thing anybody
    # working in these repositories eventually has to do. A partial clone fetches blobs on
    # demand and keeps that possible.
    git clone --filter=blob:none "$url" "$dest"
  fi

  echo "==> $dir: fetching $rev"
  git -C "$dest" fetch --tags origin
  # Detached, deliberately: this checkout is at a *pin*, and a branch name here invites a
  # commit on top that the lock does not describe. Work on the fork starts with an explicit
  # `git checkout cide`.
  git -C "$dest" checkout --detach "$rev"
  echo "    $dir at $(git -C "$dest" rev-parse --short HEAD) ($rev)"
done

echo
echo "All three forks are at their pins. \`cargo xtask package --check\` will confirm."
