#!/usr/bin/env bash
# Same shell-sourceable pins and ref-name lookup as the release workflow; no clones or builds.
set -euo pipefail
cd "$(dirname "$0")/.."
. packaging/rust-analyzer.lock
. packaging/salsa.lock
. packaging/gopls.lock

failed=''
check() {
  local what="$1" url="$2" rev="$3" refs
  # Preserve transport errors too, so a failed network request cannot look like a resolved pin.
  if refs=$(GIT_TERMINAL_PROMPT=0 git ls-remote "$url" "$rev") && [ -n "$refs" ]; then
    printf 'ok: %s -> %s @ %s\n' "$what" "$url" "$rev"
  else
    printf 'packaging/%s.lock: %s does not resolve in %s\n' "$what" "$rev" "$url" >&2
    failed="$failed $what"
  fi
}
check rust-analyzer "$CIDE_RA_URL" "$CIDE_RA_REV"
check salsa "$CIDE_SALSA_URL" "$CIDE_SALSA_REV"
check gopls "$CIDE_GOPLS_URL" "$CIDE_GOPLS_REV"
if [ -n "$failed" ]; then
  printf 'Unreachable fork pins:%s\n' "$failed" >&2
  exit 1
fi
