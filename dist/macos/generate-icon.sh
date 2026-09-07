#!/bin/bash
# Requires librsvg (brew install librsvg) and Apple's iconutil.
set -euo pipefail
readonly SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ $# -gt 1 || ( $# -eq 1 && "$1" != "--check" ) ]]; then
  echo "usage: $0 [--check]" >&2
  exit 2
fi
for tool in rsvg-convert iconutil; do
  command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done
readonly WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/AppIcon.iconset"
# Render each representation directly from the vector, including Retina sizes.
for points in 16 32 128 256 512; do
  for scale in 1 2; do
    suffix=""
    [[ "$scale" == 1 ]] || suffix="@2x"
    pixels=$((points * scale))
    rsvg-convert --width "$pixels" --height "$pixels" \
      --output "$WORK/AppIcon.iconset/icon_${points}x${points}${suffix}.png" \
      "$SCRIPT_DIR/AppIcon.svg"
  done
done
iconutil --convert icns --output "$WORK/AppIcon.icns" "$WORK/AppIcon.iconset"
if [[ "${1:-}" == "--check" ]]; then
  cmp "$WORK/AppIcon.icns" "$SCRIPT_DIR/AppIcon.icns" || {
    echo "AppIcon.icns is stale; run $0" >&2
    exit 1
  }
  echo "AppIcon.icns matches the source artwork at all ten macOS representations."
else
  cp "$WORK/AppIcon.icns" "$SCRIPT_DIR/AppIcon.icns"
fi
