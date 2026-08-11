#!/usr/bin/env bash
#
# Capture the running cide window.
#
# There is no way to look at this app from here otherwise. It is a WebKitGTK webview on a
# Wayland session, so every X11 capture tool sees nothing: KDE's compositor does not expose
# native Wayland surfaces to X11 clients, and an `import`/`scrot` grab returns the XWayland
# layer, which cide is not part of. That is also why `tauri-mcp` cannot help here — its
# screenshot path goes through an X11 crate.
#
# So this uses the tools that do work on KDE Wayland, in preference order:
#   1. spectacle -a  — KDE's own, goes through the compositor, captures the active window
#   2. grim          — wlroots protocol; present on this machine, may not bind under KWin
#
# Usage:
#   scripts/shot.sh [output.png]      capture the ACTIVE window (focus cide first)
#   scripts/shot.sh -f [output.png]   capture the whole screen
#
# Exits non-zero with a reason rather than writing an empty or black file: a screenshot that
# silently captured nothing is worse than none, because it gets looked at and believed.
set -euo pipefail

full=0
if [ "${1-}" = "-f" ]; then
  full=1
  shift
fi

out="${1-/tmp/cide-shot.png}"
rm -f "$out"

if command -v spectacle >/dev/null 2>&1; then
  # `-b` background, `-n` no notification popup — without both, Spectacle opens its own window
  # and that window is what ends up in front of the one being captured.
  if [ "$full" = 1 ]; then
    spectacle -b -n -f -o "$out" >/dev/null 2>&1 || true
  else
    spectacle -b -n -a -o "$out" >/dev/null 2>&1 || true
  fi
fi

# grim only as a fallback, and only whole-screen: it has no concept of "active window" without
# a compositor-specific helper, and KWin does not implement the wlroots protocol it wants.
if [ ! -s "$out" ] && command -v grim >/dev/null 2>&1; then
  grim "$out" >/dev/null 2>&1 || true
fi

if [ ! -s "$out" ]; then
  echo "[shot] no screenshot produced." >&2
  echo "[shot] Is cide running, and is its window focused? On KDE Wayland the capture goes" >&2
  echo "[shot] through the compositor, so a headless or unfocused session yields nothing." >&2
  exit 1
fi

# A capture that is a single flat colour is the classic Wayland-through-X11 failure, and it is
# indistinguishable from a real screenshot until someone looks at it. Say so instead.
if command -v identify >/dev/null 2>&1; then
  colors=$(identify -format '%k' "$out" 2>/dev/null || echo 2)
  if [ "$colors" -le 1 ]; then
    echo "[shot] captured $out but it holds one colour — the compositor gave us a blank frame." >&2
    exit 1
  fi
fi

echo "$out"
