#!/bin/bash
set -euo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
readonly REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
readonly TOOLS_DIR="$REPO_ROOT/crates/sequencer/tools"
readonly DGEN_TOOL="$TOOLS_DIR/DGenLisp-macos-arm64"
readonly DGEN_TOOLCHAIN="$TOOLS_DIR/dgen-toolchain"
readonly TOOLCHAIN_VERSION="$DGEN_TOOLCHAIN/VERSION.json"
readonly CONTENT_DIR="$REPO_ROOT/content"
readonly ICON="$SCRIPT_DIR/AppIcon.icns"
readonly FONT_DIR="$SCRIPT_DIR/fonts"
readonly UI_FONT="$FONT_DIR/JetBrainsMono-Regular.ttf"
readonly UI_FONT_LICENSE="$FONT_DIR/OFL.txt"
readonly UI_FONT_SHA256="a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f"
readonly OUTPUT_DIR="$REPO_ROOT/dist/out"
readonly BUNDLE_ID="com.universalsequences.eseq"
readonly MINIMUM_MACOS="11.0"

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

if [[ ! -d "$DGEN_TOOLCHAIN" || ! -f "$TOOLCHAIN_VERSION" ]]; then
  fail "staged DGen toolchain not found at $DGEN_TOOLCHAIN. Run ./rebuild_dgenlisp_tool.sh to build it before packaging."
fi
if [[ ! -x "$DGEN_TOOL" ]]; then
  fail "DGenLisp compiler not found at $DGEN_TOOL. Run ./scripts/fetch_dgenlisp.sh --target macos-arm64 before packaging."
fi
[[ -d "$CONTENT_DIR" ]] || fail "factory content not found at $CONTENT_DIR"
[[ -f "$ICON" ]] || fail "app icon not found at $ICON"
[[ -f "$UI_FONT" ]] || fail "application font not found at $UI_FONT"
[[ -f "$UI_FONT_LICENSE" ]] || fail "application font license not found at $UI_FONT_LICENSE"

[[ "$(uname -s)" == "Darwin" ]] || fail "macOS packaging must run on macOS"
[[ "$(uname -m)" == "arm64" ]] || fail "ESeq R1 supports Apple Silicon (arm64) only"

for command in cargo codesign ditto git hdiutil plutil shasum strings xattr; do
  require_command "$command"
done

readonly ACTUAL_UI_FONT_SHA256="$(shasum -a 256 "$UI_FONT" | awk '{print $1}')"
[[ "$ACTUAL_UI_FONT_SHA256" == "$UI_FONT_SHA256" ]] || \
  fail "application font checksum mismatch: expected $UI_FONT_SHA256, got $ACTUAL_UI_FONT_SHA256"

readonly TOOLCHAIN_TARGET="$(plutil -extract target raw "$TOOLCHAIN_VERSION")"
readonly TOOLCHAIN_MINIMUM_MACOS="$(plutil -extract minimum_macos raw "$TOOLCHAIN_VERSION")"
[[ "$TOOLCHAIN_TARGET" == "arm64-apple-macos" ]] || \
  fail "staged toolchain targets $TOOLCHAIN_TARGET; expected arm64-apple-macos"
[[ "$TOOLCHAIN_MINIMUM_MACOS" == "$MINIMUM_MACOS" ]] || \
  fail "staged toolchain requires macOS $TOOLCHAIN_MINIMUM_MACOS; expected $MINIMUM_MACOS"

# The identifier is ratified once (release spec section 3.1) and appears both
# here and as the installed data directory in the Release path arm. A silent
# divergence would split a tester's library across two directories.
readonly APP_PATHS_SRC="$REPO_ROOT/crates/sequencer/src/app_paths/mod.rs"
grep -q "RELEASE_DATA_DIRECTORY: &str = \"$BUNDLE_ID\";" "$APP_PATHS_SRC" || \
  fail "RELEASE_DATA_DIRECTORY in $APP_PATHS_SRC does not match CFBundleIdentifier $BUNDLE_ID"

readonly VERSION="$(awk '
  /^\[package\]$/ { in_package = 1; next }
  in_package && /^\[/ { exit }
  in_package && /^[[:space:]]*version[[:space:]]*=/ {
    value = $0
    sub(/^[^=]*=[[:space:]]*/, "", value)
    gsub(/[[:space:]\"]/, "", value)
    print value
    exit
  }
' "$REPO_ROOT/crates/sequencer/Cargo.toml")"
[[ -n "$VERSION" ]] || fail "could not read the sequencer crate version"

readonly GIT_HASH="$(git -C "$REPO_ROOT" rev-parse --short=8 HEAD)"
readonly ARTIFACT_NAME="ESeq-$VERSION-$GIT_HASH"

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  if [[ "$CARGO_TARGET_DIR" = /* ]]; then
    readonly TARGET_DIR="$CARGO_TARGET_DIR"
  else
    readonly TARGET_DIR="$REPO_ROOT/$CARGO_TARGET_DIR"
  fi
else
  # Deliberately not the shared "target/": the path remapping below is part of
  # the RUSTFLAGS fingerprint, so sharing a directory with ordinary
  # `cargo build --release` would make the two rebuild each other every time.
  readonly TARGET_DIR="$REPO_ROOT/target/package"
fi
export CARGO_TARGET_DIR="$TARGET_DIR"
readonly METAL_SEQ="$TARGET_DIR/release/metal_seq"
readonly ESEQ_CLI="$TARGET_DIR/release/eseq"

mkdir -p "$OUTPUT_DIR"
readonly WORK_DIR="$(mktemp -d "$OUTPUT_DIR/.build.XXXXXX")"
cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT HUP INT TERM

readonly APP="$WORK_DIR/ESeq.app"
readonly CONTENTS="$APP/Contents"
readonly MACOS="$CONTENTS/MacOS"
readonly RESOURCES="$CONTENTS/Resources"
readonly DMG_ROOT="$WORK_DIR/dmg-root"
readonly DMG="$WORK_DIR/$ARTIFACT_NAME.dmg"

printf 'Building ESeq %s (%s)\n' "$VERSION" "$GIT_HASH"
printf 'Staged DGen toolchain metadata:\n'
cat "$TOOLCHAIN_VERSION"
printf '\n'

cd "$REPO_ROOT"
# A shipped binary must not disclose the build machine. Without this, rustc
# bakes an absolute panic/debuginfo path for every crate it compiles -- the
# checkout, the Cargo registry, and the target directory -- and the audit
# below rejects the bundle. The replacements are opaque roots, not real paths.
readonly CARGO_ROOT="${CARGO_HOME:-$HOME/.cargo}"
readonly RUSTUP_ROOT="${RUSTUP_HOME:-$HOME/.rustup}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$TARGET_DIR=/build --remap-path-prefix=$REPO_ROOT=/eseq --remap-path-prefix=$CARGO_ROOT=/cargo --remap-path-prefix=$RUSTUP_ROOT=/rustup"
# Replaces the dev-only `env!("ESEQ_DEV_MANIFEST_DIR")` checkout literals with a
# synthetic root (see the build scripts). Every consumer is a fallback guarded
# by an existence check, so they simply stop resolving, which is the packaged
# behavior anyway.
export ESEQ_PACKAGED=1
cargo build --release -p sequencer --bin metal_seq --bin eseq
[[ -x "$METAL_SEQ" ]] || fail "Cargo did not produce $METAL_SEQ"
[[ -x "$ESEQ_CLI" ]] || fail "Cargo did not produce $ESEQ_CLI"

mkdir -p "$MACOS" "$RESOURCES"
ditto "$METAL_SEQ" "$MACOS/metal_seq"
ditto "$ESEQ_CLI" "$MACOS/eseq"
ditto "$DGEN_TOOL" "$MACOS/DGenLisp-macos-arm64"
ditto "$DGEN_TOOLCHAIN" "$RESOURCES/dgen-toolchain"
ditto "$CONTENT_DIR" "$RESOURCES"
ditto "$ICON" "$RESOURCES/AppIcon.icns"
ditto "$FONT_DIR" "$RESOURCES/fonts"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>ESeq</string>
  <key>CFBundleExecutable</key>
  <string>metal_seq</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>ESeq</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$GIT_HASH</string>
  <key>LSArchitecturePriority</key>
  <array>
    <string>arm64</string>
  </array>
  <key>LSMinimumSystemVersion</key>
  <string>$MINIMUM_MACOS</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST
plutil -lint "$CONTENTS/Info.plist"

# Downloaded factory files and tools can carry quarantine or Finder metadata.
# Neither belongs in a release bundle, and resource forks make codesign reject it.
xattr -cr "$APP"

# Sign the outer bundle last so its resource seal covers the completed tree.
# Cargo and the staged tools already carry linker/ad-hoc signatures.
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --deep --strict "$APP"

# A release artifact must not disclose or depend on this checkout. Audit every
# executable payload, not just CFBundleExecutable.
while IFS= read -r -d '' executable; do
  if strings "$executable" | grep -F -e "$REPO_ROOT" -e "$HOME/" >/dev/null; then
    fail "build-machine path found in shipped executable: $executable"
  fi
done < <(find "$MACOS" "$RESOURCES/dgen-toolchain/bin" -type f -perm -111 -print0)

mkdir -p "$DMG_ROOT"
ditto "$APP" "$DMG_ROOT/ESeq.app"
ln -s /Applications "$DMG_ROOT/Applications"
hdiutil create \
  -volname "ESeq" \
  -srcfolder "$DMG_ROOT" \
  -format UDZO \
  -ov \
  "$DMG"

rm -rf "$OUTPUT_DIR/ESeq.app"
ditto "$APP" "$OUTPUT_DIR/ESeq.app"
mv -f "$DMG" "$OUTPUT_DIR/$ARTIFACT_NAME.dmg"

printf '\nCreated:\n  %s\n  %s\n' \
  "$OUTPUT_DIR/ESeq.app" \
  "$OUTPUT_DIR/$ARTIFACT_NAME.dmg"
