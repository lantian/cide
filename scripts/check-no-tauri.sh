#!/usr/bin/env bash
# The structural CI gate, shared with ./test.sh. Match crate names, never checkout paths.
set -euo pipefail
cd "$(dirname "$0")/.."

webview_deps() {
  local tree
  # Propagate cargo errors explicitly even inside command substitution: an empty failed
  # cargo tree must never be interpreted as a domain crate with no webview dependencies.
  tree=$(cargo tree --locked -p "$1" --edges normal,build,dev --prefix none --no-dedupe) || return
  awk '{print $1}' <<< "$tree" | sort -u | grep -E '^(tauri|wry|tao)(-|$)' || true
}

app_deps=$(webview_deps cide-app)
if [ -z "$app_deps" ]; then
  echo 'The webview detector matched nothing in cide-app; refusing a vacuous check.' >&2
  exit 1
fi

members=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,os,sys; md=json.load(sys.stdin); print("\n".join(p["name"] + " " + os.path.relpath(p["manifest_path"], md["workspace_root"]) for p in md["packages"]))')
checked=0
violations=''
while read -r name manifest; do
  [ -n "$name" ] || continue
  [ "$name" != cide-app ] || continue
  checked=$((checked + 1))
  found=$(webview_deps "$name")
  if [ -n "$found" ]; then
    violations="$violations $name"
    printf '%s: %s pulls in the webview stack: %s\n' "$manifest" "$name" "$found" >&2
  fi
done <<< "$members"
if [ "$checked" -eq 0 ]; then
  echo 'No domain workspace members found; refusing an empty structural check.' >&2
  exit 1
fi
if [ -n "$violations" ]; then
  printf 'Only cide-app may depend on tauri. Offenders:%s\n' "$violations" >&2
  exit 1
fi
printf 'ok: %s workspace members checked, tauri confined to cide-app\n' "$checked"
