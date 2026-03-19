#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TRACE_DIR="${TRACE_DIR:-$ROOT_DIR/profiles}"
BUILD_ROOT="${BUILD_ROOT:-$TRACE_DIR/builds}"
STAMP="$(date +%Y%m%d-%H%M%S)"
TRACE_PATH="${1:-$TRACE_DIR/eseqlisp-metal-$STAMP.trace}"
SNAPSHOT_DIR="$BUILD_ROOT/$STAMP"
BIN_NAME="eseqlisp"
TARGET_BIN="$ROOT_DIR/target/debug/$BIN_NAME"
TARGET_DSYM="$ROOT_DIR/target/debug/$BIN_NAME.dSYM"
SNAPSHOT_BIN="$SNAPSHOT_DIR/$BIN_NAME"
SNAPSHOT_DSYM="$SNAPSHOT_DIR/$BIN_NAME.dSYM"

mkdir -p "$TRACE_DIR" "$BUILD_ROOT" "$SNAPSHOT_DIR"

if ! command -v xcrun >/dev/null 2>&1; then
  echo "error: xcrun not found; install Xcode command line tools" >&2
  exit 1
fi

echo "Building debug binary with stable symbol settings..."
(
  cd "$ROOT_DIR"
  CARGO_INCREMENTAL=0 \
  RUSTFLAGS="-C debuginfo=2 -C split-debuginfo=unpacked" \
  cargo build
)

if [[ ! -x "$TARGET_BIN" ]]; then
  echo "error: built binary not found at $TARGET_BIN" >&2
  exit 1
fi

echo "Generating dSYM..."
rm -rf "$TARGET_DSYM"
dsymutil "$TARGET_BIN" -o "$TARGET_DSYM"

echo "Freezing binary snapshot..."
cp "$TARGET_BIN" "$SNAPSHOT_BIN"
rm -rf "$SNAPSHOT_DSYM"
cp -R "$TARGET_DSYM" "$SNAPSHOT_DSYM"

echo "Snapshot UUIDs:"
dwarfdump --uuid "$SNAPSHOT_BIN" "$SNAPSHOT_DSYM"
echo
echo "Recording Time Profiler trace to:"
echo "  $TRACE_PATH"
echo
echo "Binary snapshot:"
echo "  $SNAPSHOT_BIN"
echo "dSYM snapshot:"
echo "  $SNAPSHOT_DSYM"
echo
echo "The trace will stop when eseqlisp exits."

xcrun xctrace record \
  --template "Time Profiler" \
  --output "$TRACE_PATH" \
  --launch -- \
  "$SNAPSHOT_BIN" --metal

echo
echo "Trace written to:"
echo "  $TRACE_PATH"
echo "Binary snapshot:"
echo "  $SNAPSHOT_BIN"
echo "dSYM snapshot:"
echo "  $SNAPSHOT_DSYM"
echo
echo "Open trace with:"
echo "  open \"$TRACE_PATH\""
