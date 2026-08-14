#!/usr/bin/env bash
#
# Rasterise crates/cide-app/icons/icon.svg to every PNG the bundlers install.
#
#   scripts/gen-icons.sh            rewrite the PNGs
#   scripts/gen-icons.sh --check    fail if any PNG has drifted from the SVG
#
# ## Why a script and not a checked-in set of hand-exported PNGs
#
# The icon exists at seven sizes. A hand-exported set drifts the first time somebody nudges
# the artwork and re-exports four of the seven, and the failure is invisible: the launcher
# picks the size closest to its cell, so the wrong drawing appears at one zoom level on one
# desktop and nowhere else. `--check` is what makes that impossible; run it in CI next to
# `cargo xtask codegen --check`, which it is deliberately shaped like.
#
# ## Why each size is rendered from the SVG instead of resizing a master PNG

#
# The artwork's straight edges are all on multiples of 4 in a 64-unit viewBox, so at every
# size below they land on whole pixels. Rendering directly keeps that; rendering a 128px
# master and downsampling with Lanczos throws it away — compared side by side at 16px, the
# downsampled cursor bar has a two-pixel soft edge where the direct render has a hard one.
# ImageMagick is still used, but for comparison and metadata, not for scaling the artwork.
#
# ## Why the renderer is not just `magick`
#
# ImageMagick has no SVG coder of its own; it shells out to `rsvg-convert`, and this
# machine's policy.xml refuses the SVG delegate outright:
#
#   magick: attempt to perform an operation not authorized by the security policy `SVG'
#
# So a real SVG renderer has to be found first. Preference order below is quality and
# reproducibility: rsvg-convert (librsvg, what GTK itself draws with), then Inkscape, then
# ImageMagick as a last resort in case a machine has the delegate enabled.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SVG="$ROOT/crates/cide-app/icons/icon.svg"
OUT="$ROOT/crates/cide-app/icons"

# name:size. Every entry below is listed in `bundle.icon` in crates/cide-app/tauri.conf.json,
# and that is load-bearing: tauri-bundler installs exactly the files that array names and
# nothing else. When the array held only 32, 128 and 512, the other four rasters here were
# generated, committed and drift-checked while reaching no desktop at all — GTK and Qt were
# left to scale 32 down to 16 themselves, which is precisely the mud the 16px drawing was
# tuned to avoid. Add a size here and it must be added there too.
#
# `icon.png` is 512 and keeps its Tauri name: the AppImage bundler picks the largest square
# icon for `.DirIcon`, so this is the one a file manager shows for the download.
#
# It is also listed FIRST in `bundle.icon`, and that ordering is load-bearing rather than
# tidy. tauri-codegen's `find_icon` embeds `default_window_icon` from the FIRST `.png` in
# that array (tauri-codegen/src/context.rs), and that icon is what the running window hands
# to KDE for the task manager, the window list and Alt-Tab. Listing the sizes small-first
# would embed the 16px raster and leave KDE upscaling it to 48 and 128 — the worst possible
# source for the exact surface the artwork was drawn for. Largest first, so every consumer
# scales down from the best raster instead of up from the worst.
# Nothing else reads the order: the freedesktop installer keys on each PNG's own pixel
# dimensions and the AppImage picks the largest square, both order-independent.
#
# The freedesktop hicolor sizes are spelled `WxH.png` because tauri-bundler derives the
# install directory from the file's pixel dimensions, not from its name
# (bundle/linux/freedesktop/mod.rs, `list_icon_files`), so the names are for humans.
#
# 128x128@2x.png is deliberately absent. It used to sit here as a placeholder, nothing in
# the repository referenced it, and tauri-bundler's `is_retina` would have installed it to
# `usr/share/icons/hicolor/128x128@2/apps/`, a directory the icon theme specification does
# not define and no theme searches. 256x256.png carries that resolution to the place the
# spec actually looks.
SIZES=(
  "16x16.png:16"
  "32x32.png:32"
  "48x48.png:48"
  "64x64.png:64"
  "128x128.png:128"
  "256x256.png:256"
  "icon.png:512"
)

MODE="write"
if [[ "${1:-}" == "--check" ]]; then
  MODE="check"
elif [[ $# -gt 0 ]]; then
  echo "usage: $(basename "$0") [--check]" >&2
  exit 2
fi

render() { # render <size> <destination>
  local size="$1" dest="$2"
  if command -v rsvg-convert >/dev/null 2>&1; then
    rsvg-convert --width="$size" --height="$size" --format=png --output="$dest" "$SVG"
  elif command -v inkscape >/dev/null 2>&1; then
    # --export-background-opacity=0 or Inkscape composites the document background, which
    # for an app icon means a white square on every dark panel in the world.
    inkscape --export-type=png --export-filename="$dest" \
             --export-width="$size" --export-height="$size" \
             --export-background-opacity=0 "$SVG" >/dev/null 2>&1
  elif magick -list format 2>/dev/null | grep -qi '^ *MSVG'; then
    magick -background none "$SVG" -resize "${size}x${size}" "$dest"
  else
    echo "gen-icons: no SVG renderer found (tried rsvg-convert, inkscape, magick)" >&2
    exit 1
  fi
  # Strip timestamps and text chunks so two runs on one machine are byte-identical and a
  # regenerated file is not a diff on its own.
  magick "$dest" -strip -define png:color-type=6 "$dest"
}

if ! command -v magick >/dev/null 2>&1; then
  echo "gen-icons: ImageMagick (magick) is required" >&2
  exit 1
fi

# Normalised RMSE between two rasters; empty when ImageMagick refuses the pair outright.
rmse_of() {
  local raw
  raw="$(magick compare -metric RMSE "$1" "$2" null: 2>&1 >/dev/null || true)"
  printf '%s' "$raw" | sed -n 's/.*(\([0-9.e-]*\)).*/\1/p'
}

# Everything strictly inside the artwork: the antialiased rim eroded away, everything
# outside it forced to black. Both rasters are masked with the SAME mask, taken from the
# committed file, so what is left compares the colour of the solid interior and nothing
# else. Erode and not a plain alpha threshold, because a threshold still keeps the pixel
# just inside the edge, which is exactly the one two renderers disagree about.
interior() { # interior <raster> <mask> <destination>
  magick "$1" "$2" -alpha off -compose CopyOpacity -composite \
         -background black -alpha remove -alpha off "$3"
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

status=0
for entry in "${SIZES[@]}"; do
  name="${entry%%:*}"
  size="${entry##*:}"
  dest="$OUT/$name"

  if [[ "$MODE" == "write" ]]; then
    render "$size" "$dest"
    printf 'wrote  %s (%sx%s)\n' "$name" "$size" "$size"
    continue
  fi

  if [[ ! -f "$dest" ]]; then
    printf 'MISSING %s\n' "$name"
    status=1
    continue
  fi
  # Dimensions first, and separately from the pixel comparison below. `magick compare`
  # happily compares a 47x47 against a 48x48 by looking at the overlap and can return an
  # RMSE under the tolerance, which would let a wrong-sized file through — and the size is
  # the one property tauri-bundler reads, since it derives the hicolor install directory
  # from it.
  actual="$(magick identify -format '%wx%h' "$dest")"
  if [[ "$actual" != "${size}x${size}" ]]; then
    printf 'DIFFERS %s (is %s, must be %sx%s)\n' "$name" "$actual" "$size" "$size"
    status=1
    continue
  fi

  render "$size" "$TMP/$name"

  # Two comparisons, because one number provably cannot do both jobs.
  #
  # Whole-image RMSE catches geometry: a moved shape, a changed stroke width, a wrong
  # corner radius. It has to keep a loose tolerance, because two renderers (or two librsvg
  # releases) disagree along every antialiased edge, and `cmp` would turn a toolchain
  # upgrade into a red build claiming the artwork changed when it did not.
  #
  # That loose tolerance is blind to colour, and this was measured rather than assumed:
  # deleting the middle gradient stop — reverting the ramp to the straight two-stop version
  # icon.svg explicitly rejects, which doubles the share of the mark under 3:1 on white —
  # moves whole-image RMSE to only 0.0115, under any tolerance wide enough for
  # antialiasing. A check that waves through the one edit its own artwork comments argue
  # about is not a check. So colour is tested separately, over the eroded interior, where
  # no antialiased pixel survives and the tolerance can therefore be tight.
  geometry="$(rmse_of "$dest" "$TMP/$name")"
  if [[ -z "$geometry" ]]; then
    printf 'DIFFERS %s (dimensions differ from the SVG at %s)\n' "$name" "$size"
    status=1
    continue
  fi
  if awk -v v="$geometry" 'BEGIN { exit !(v > 0.02) }'; then
    printf 'DIFFERS %s (geometry RMSE %s > 0.02; re-run scripts/gen-icons.sh)\n' \
           "$name" "$geometry"
    status=1
    continue
  fi

  # Only where there IS an interior. Eroding 2px off the 16px raster, whose caret stroke is
  # 3px wide, would leave nothing to compare and the test would pass by being empty — the
  # exact failure mode this whole block exists to remove. Every size is rendered from the
  # same SVG, so a colour edit caught at 128 and above is a colour edit caught everywhere.
  colour="skipped"
  if [[ "$size" -ge 128 ]]; then
    magick "$dest" -alpha extract -threshold 99% \
           -morphology Erode Octagon:2 "$TMP/mask-$name"
    interior "$dest" "$TMP/mask-$name" "$TMP/ref-$name"
    interior "$TMP/$name" "$TMP/mask-$name" "$TMP/new-$name"
    colour="$(rmse_of "$TMP/ref-$name" "$TMP/new-$name")"
    if [[ -z "$colour" ]]; then
      printf 'DIFFERS %s (interior could not be compared)\n' "$name"
      status=1
      continue
    fi
    if awk -v v="$colour" 'BEGIN { exit !(v > 0.004) }'; then
      printf 'DIFFERS %s (interior colour RMSE %s > 0.004; re-run scripts/gen-icons.sh)\n' \
             "$name" "$colour"
      status=1
      continue
    fi
  fi

  printf 'ok      %s (geometry %s, colour %s)\n' "$name" "$geometry" "$colour"
done

exit "$status"
